//! Shared Chat domain models, helpers, preferences, QR encoder, and small UI primitives.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

use crate::api;
use crate::dates::{local_date, local_time};
use crate::icons::Icon;
use crate::pim::avatar_hue;
use crate::shared::Preferences;
use crate::ui::{Dialog, MenuAction, window};

pub const BASE: &str = "/api/v1/chat";

pub const CHAT_LIMITS_ACCOUNTS: usize = 10;
pub const CHAT_LIMITS_NAME: usize = 64;
pub const CHAT_LIMITS_TEXT: usize = 65536;
pub const CHAT_LIMITS_DISCORD_TEXT: usize = 4000;

pub const CHAT_QUICK_REACTIONS: [&str; 6] = [
    "\u{1f44d}",
    "\u{2764}\u{fe0f}",
    "\u{1f602}",
    "\u{1f62e}",
    "\u{1f622}",
    "\u{1f64f}",
];

pub const CHAT_EMOJI_PALETTE: [&str; 12] = [
    "\u{1f44d}",
    "\u{2764}\u{fe0f}",
    "\u{1f602}",
    "\u{1f525}",
    "\u{1f389}",
    "\u{1f64f}",
    "\u{1f440}",
    "\u{1f914}",
    "\u{1f60a}",
    "\u{1f62e}",
    "\u{1f622}",
    "\u{2705}",
];

/// Mute choices as WhatsApp offers them; -1 is always.
pub const CHAT_MUTE_CHOICES: [(&str, i64); 3] = [
    ("8 hours", 8 * 3_600_000),
    ("1 week", 7 * 86_400_000),
    ("Always", -1),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatView {
    Chats,
    Unread,
    Groups,
    Archived,
}

impl ChatView {
    pub const ALL: [Self; 4] = [Self::Chats, Self::Unread, Self::Groups, Self::Archived];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chats => "chats",
            Self::Unread => "unread",
            Self::Groups => "groups",
            Self::Archived => "archived",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "unread" => Self::Unread,
            "groups" => Self::Groups,
            "archived" => Self::Archived,
            _ => Self::Chats,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Chats => "Chats",
            Self::Unread => "Unread",
            Self::Groups => "Groups",
            Self::Archived => "Archived",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Chats => "chat",
            Self::Unread => "unread",
            Self::Groups => "contacts",
            Self::Archived => "archive",
        }
    }
}

/// Quick filter chips shown in WhatsApp's `Chats` list without changing the sidebar view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickFilter {
    All,
    Unread,
    Pinned,
    Groups,
}

impl QuickFilter {
    pub const ALL: [Self; 4] = [Self::All, Self::Unread, Self::Pinned, Self::Groups];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Unread => "Unread",
            Self::Pinned => "Pinned",
            Self::Groups => "Groups",
        }
    }

    pub fn matches(self, thread: &ChatThread) -> bool {
        match self {
            Self::All => true,
            Self::Unread => thread.unread > 0,
            Self::Pinned => thread.pinned,
            Self::Groups => thread.kind == "group",
        }
    }
}

/// Chat display and composer preferences stored in `/api/v1/pim/preferences`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatPrefs {
    pub density: String,
    pub send_key: String,
    pub format_markdown: bool,
    pub show_emoji: bool,
}

impl Default for ChatPrefs {
    fn default() -> Self {
        Self {
            density: "comfortable".into(),
            send_key: "enter".into(),
            format_markdown: true,
            show_emoji: true,
        }
    }
}

impl ChatPrefs {
    pub fn from_preferences(prefs: Option<&Preferences>) -> Self {
        let mut out = Self::default();
        let Some(prefs) = prefs else {
            return out;
        };
        if let Some(density) = prefs.rest.get("chatDensity").and_then(|v| v.as_str())
            && matches!(density, "comfortable" | "compact")
        {
            out.density = density.to_owned();
        }
        if let Some(send_key) = prefs.rest.get("chatSendKey").and_then(|v| v.as_str())
            && matches!(send_key, "enter" | "modEnter")
        {
            out.send_key = send_key.to_owned();
        }
        if let Some(fmt) = prefs.rest.get("chatFormatMarkdown").and_then(|v| v.as_bool()) {
            out.format_markdown = fmt;
        }
        if let Some(emoji) = prefs.rest.get("chatShowEmoji").and_then(|v| v.as_bool()) {
            out.show_emoji = emoji;
        }
        out
    }

    pub fn is_compact(&self) -> bool {
        self.density == "compact"
    }

    pub fn send_on_mod(&self) -> bool {
        self.send_key == "modEnter"
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LightboxItem {
    pub url: String,
    pub download_url: String,
    pub file_name: String,
    pub sender: String,
    pub time: String,
    pub caption: String,
    pub message: Option<ChatMessage>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatCounts {
    pub chats: u32,
    pub unread: u32,
    pub groups: u32,
    pub archived: u32,
}

impl ChatCounts {
    pub fn for_view(&self, view: ChatView) -> u32 {
        match view {
            ChatView::Chats => self.chats,
            ChatView::Unread => self.unread,
            ChatView::Groups => self.groups,
            ChatView::Archived => self.archived,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatAccount {
    pub id: String,
    pub platform: String,
    pub name: String,
    pub jid: String,
    pub state: String,
    pub detail: String,
    pub created_at: f64,
    pub linked_at: f64,
    pub synced_at: f64,
    pub history_progress: u32,
    pub importing: bool,
    pub unread: u32,
    pub counts: ChatCounts,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatBridgeStatus {
    pub available: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatOverview {
    pub accounts: Vec<ChatAccount>,
    pub bridge: ChatBridgeStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatContact {
    pub name: String,
    pub book: String,
    pub href: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatThread {
    pub id: String,
    pub account_id: String,
    pub jid: String,
    pub kind: String,
    pub name: String,
    pub at: f64,
    pub preview: String,
    pub preview_kind: String,
    pub preview_from_me: bool,
    pub preview_sender: String,
    pub preview_status: String,
    pub unread: u32,
    pub archived: bool,
    pub pinned: bool,
    pub muted_until: f64,
    pub whatsapp_name: String,
    pub contact: Option<ChatContact>,
    pub server_id: String,
    pub section: String,
    pub order: String,
    pub topic: String,
    pub mentions: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatServer {
    pub id: String,
    pub server_id: String,
    pub name: String,
    pub muted_until: f64,
    pub unread: bool,
    pub mentions: u32,
    pub channels: Vec<ChatThread>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatFriend {
    pub id: String,
    pub name: String,
    pub username: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatVoiceMember {
    pub id: String,
    pub name: String,
    pub muted: bool,
    pub deafened: bool,
    pub video: bool,
    pub streaming: bool,
}

pub type ChatVoiceServers = BTreeMap<String, BTreeMap<String, Vec<ChatVoiceMember>>>;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatVoiceCall {
    pub state: String,
    pub server_id: String,
    pub channel_id: String,
    pub self_mute: bool,
    pub self_deaf: bool,
    pub can_speak: bool,
    pub e2ee: bool,
    pub protocol: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatVoiceEnded {
    pub channel_id: String,
    pub reason: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatLiveMessage {
    Hello {
        #[serde(default)]
        servers: ChatVoiceServers,
        #[serde(default)]
        call: Option<ChatVoiceCall>,
    },
    Voice {
        #[serde(default)]
        servers: ChatVoiceServers,
        #[serde(default)]
        complete: bool,
    },
    Call {
        #[serde(default)]
        call: Option<ChatVoiceCall>,
        #[serde(default)]
        ended: Option<ChatVoiceEnded>,
    },
    Owner {
        #[serde(default)]
        here: bool,
        #[serde(default)]
        elsewhere: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatThreadPage {
    pub pinned: Vec<ChatThread>,
    pub threads: Vec<ChatThread>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMedia {
    pub kind: String,
    pub mime: String,
    pub size: f64,
    pub seconds: u32,
    pub width: u32,
    pub height: u32,
    pub state: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatReaction {
    pub emoji: String,
    pub count: u32,
    pub mine: bool,
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMessage {
    pub id: String,
    pub message_id: String,
    pub sender: String,
    pub sender_name: String,
    pub from_me: bool,
    pub at: f64,
    pub kind: String,
    pub text: String,
    pub file_name: String,
    pub quoted_id: String,
    pub quoted_sender: String,
    pub quoted_text: String,
    pub status: String,
    pub edited: bool,
    pub deleted: bool,
    pub media: Option<ChatMedia>,
    pub reactions: Vec<ChatReaction>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMessagePage {
    pub thread: ChatThread,
    pub messages: Vec<ChatMessage>,
    pub before: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatLinkState {
    pub account_id: String,
    pub state: String,
    pub qr: Option<String>,
    pub expires_at: Option<f64>,
    pub code: Option<String>,
    pub reason: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfoAccount {
    pub id: String,
    pub name: String,
    pub state: String,
    pub platform: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfoServer {
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfoMedia {
    pub message_id: String,
    pub kind: String,
    pub mime: String,
    pub state: String,
    pub file_name: String,
    pub size: f64,
    pub at: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatMember {
    pub jid: String,
    pub name: String,
    pub admin: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfoGroup {
    pub name: String,
    pub topic: String,
    pub members: Vec<ChatMember>,
    pub created: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatInfo {
    pub thread: ChatThread,
    pub account: ChatInfoAccount,
    pub server: Option<ChatInfoServer>,
    pub messages: u64,
    pub complete: bool,
    pub media: Vec<ChatInfoMedia>,
    pub group: Option<ChatInfoGroup>,
    pub group_error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSetting {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted_until: Option<i64>,
}

pub type RunFuture = Pin<Box<dyn Future<Output = Result<(), String>>>>;

/// A confirmation dialog's text and the work it runs.
#[derive(Clone)]
pub struct Confirm {
    pub title: String,
    pub text: String,
    pub action: String,
    pub run: Arc<dyn Fn() -> RunFuture + Send + Sync>,
}

pub fn platform_name(platform: &str) -> &'static str {
    if platform == "discord" {
        "Discord"
    } else {
        "WhatsApp"
    }
}

pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "image" => "Photo",
        "video" => "Video",
        "audio" => "Audio",
        "voice" => "Voice message",
        "document" => "Document",
        "sticker" => "Sticker",
        "location" => "Location",
        "contact" => "Contact",
        "poll" => "Poll",
        "unsupported" => "Message not supported here yet",
        _ => "",
    }
}

pub fn kind_icon(kind: &str) -> Option<&'static str> {
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

pub fn state_text(detail: &str) -> &'static str {
    match detail {
        "replaced" => "Another copy of Dar is using this link. Unlink it and link again here.",
        "banned" => "WhatsApp has temporarily blocked this number.",
        "outdated" => "WhatsApp no longer accepts this version of the bridge. Update Dar.",
        "unlinked" => "You unlinked this account. Its chats stay here, read-only.",
        "unlinkedHere" => {
            "Dar stopped using this link. Remove Dar under Linked devices on your phone too."
        }
        "notLinked" => "This account is no longer linked.",
        "timeout" => "The code expired before it was used.",
        "passkey" => "This WhatsApp account asks for a passkey, which Dar does not support yet.",
        "authenticationFailed" => {
            "Discord no longer accepts this sign-in, after a new password or a sign-out elsewhere."
        }
        "gateway" => "Discord refused the connection.",
        _ => "",
    }
}

/// Fixed-width keys that sort newest first (`chatTimeKey`).
pub fn chat_time_key(at: f64) -> String {
    let clamped = at.max(0.0).floor() as i64;
    let inv = (9_999_999_999_999_i64 - clamped).max(0);
    format!("{inv:013}")
}

/// The name an attachment downloads and saves under (`localName`).
pub fn local_name(kind: &str, mime: &str, file_name: &str, at: f64, platform: &str) -> String {
    let given: String = file_name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == '\0' {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_owned();
    if !given.is_empty() && given != "." && given != ".." {
        return given.chars().take(200).collect();
    }
    let ext = match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        "video/3gpp" => "3gp",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/aac" => "aac",
        "application/pdf" => "pdf",
        _ => "",
    };
    let label = match kind {
        "image" => "Photo",
        "sticker" => "Sticker",
        "video" => "Video",
        "audio" => "Audio",
        "voice" => "Voice message",
        "document" => "Document",
        _ => "Attachment",
    };
    let d = js_sys::Date::new(&JsValue::from_f64(at));
    let service = if platform == "discord" {
        "Discord"
    } else {
        "WhatsApp"
    };
    let suffix = if ext.is_empty() {
        String::new()
    } else {
        format!(".{ext}")
    };
    format!(
        "{service} {label} {}-{:02}-{:02} at {:02}.{:02}.{:02}{suffix}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date(),
        d.get_hours(),
        d.get_minutes(),
        d.get_seconds()
    )
}

pub fn search_params(pairs: &[(&str, &str)]) -> String {
    let params = web_sys::UrlSearchParams::new().expect("params");
    for (k, v) in pairs {
        params.append(k, v);
    }
    params.to_string().as_string().unwrap_or_default()
}

pub fn media_url(thread_id: &str, message_id: &str) -> String {
    format!(
        "{BASE}/media?{}",
        search_params(&[("threadId", thread_id), ("messageId", message_id)])
    )
}

pub fn clock(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Display size within a box, keeping the photo's shape; sent as width/height attributes so space is kept while it loads.
pub fn fit(width: u32, height: u32, max_width: f64, max_height: f64) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (max_width as u32, (max_width * 0.75).round() as u32);
    }
    let (w, h) = (width as f64, height as f64);
    let scale = 1f64.min(max_width / w).min(max_height / h);
    (
        (w * scale).round().max(80.0) as u32,
        (h * scale).round().max(60.0) as u32,
    )
}

pub fn now() -> f64 {
    js_sys::Date::now()
}

/// A custom Discord emoji is kept as `name:id` and shown as `:name:`.
pub fn emoji_label(emoji: &str) -> String {
    if let Some((name, id)) = emoji.split_once(':')
        && !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '~' || c == '-')
        && !id.is_empty()
        && id.chars().all(|c| c.is_ascii_digit())
    {
        return format!(":{name}:");
    }
    emoji.to_owned()
}

pub fn quiet_until(until: f64) -> bool {
    until == -1.0 || until > now()
}

pub fn muted(thread: &ChatThread) -> bool {
    quiet_until(thread.muted_until)
}

pub fn initials(name: &str) -> String {
    let trimmed = name.strip_prefix('+').unwrap_or(name);
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let Some(first) = words.first() else {
        return "?".into();
    };
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

pub fn phone_of(jid: &str) -> String {
    match jid.strip_suffix("@s.whatsapp.net") {
        Some(_) => format!("+{}", jid.split('@').next().unwrap_or_default()),
        None => String::new(),
    }
}

pub fn thread_display_name(thread: &ChatThread) -> String {
    if !thread.name.is_empty() {
        thread.name.clone()
    } else {
        Some(phone_of(&thread.jid))
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "Unknown".into())
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

pub fn day_label(time: f64) -> String {
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
            local_date(
                time,
                &[("day", "numeric"), ("month", "long"), ("year", "numeric")],
            )
        } else {
            local_date(time, &[("day", "numeric"), ("month", "long")])
        }
    }
}

pub fn list_time(time: f64) -> String {
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

pub fn iso(time: f64) -> String {
    js_sys::Date::new(&time.into())
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
}

pub fn narrow() -> bool {
    window()
        .match_media("(max-width:800px)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches())
}

pub fn preview(thread: &ChatThread) -> String {
    if thread.preview_kind == "deleted" {
        return "This message was deleted".into();
    }
    let label = if thread.preview_kind.is_empty() {
        ""
    } else {
        kind_label(&thread.preview_kind)
    };
    if thread.preview.is_empty() {
        label.into()
    } else if !label.is_empty()
        && thread.preview_kind != "text"
        && thread.preview_kind != "document"
    {
        format!("{label} \u{b7} {}", thread.preview)
    } else {
        thread.preview.clone()
    }
}

pub fn message_of(cause: &api::RequestError) -> String {
    if cause.message.is_empty() {
        "The request could not be completed.".into()
    } else {
        cause.message.clone()
    }
}

pub fn window_clipboard_write(text: &str) {
    let _ = window().navigator().clipboard().write_text(text);
}

pub fn mute_menu_items(on_setting: Callback<ChatSetting>) -> Vec<MenuAction> {
    CHAT_MUTE_CHOICES
        .iter()
        .map(|(label, ms)| {
            let ms = *ms;
            MenuAction::plain(*label).run(move || {
                on_setting.try_run(ChatSetting {
                    muted_until: Some(if ms < 0 { -1 } else { now() as i64 + ms }),
                    ..Default::default()
                });
            })
        })
        .collect()
}

#[component]
pub fn ChatAvatar(
    #[prop(into)] name: String,
    group: bool,
    #[prop(optional, into)] size: String,
) -> impl IntoView {
    let class = format!("chat-avatar contact-initials {} {size}", avatar_hue(&name));
    view! {
        <span class=class aria-hidden="true">
            {if group {
                view! { <Icon name="contacts"/> }.into_any()
            } else {
                initials(&name).into_any()
            }}
        </span>
    }
}

#[component]
pub fn Ticks(#[prop(into)] status: String) -> impl IntoView {
    match status.as_str() {
        "pending" => view! {
            <span class="chat-tick" title="Sending"><Icon name="clock"/><span class="sr-only">"Sending"</span></span>
        }.into_any(),
        "failed" => view! {
            <span class="chat-tick is-failed" title="Not sent"><Icon name="alert"/><span class="sr-only">"Not sent"</span></span>
        }.into_any(),
        "sent" => view! {
            <span class="chat-tick" title="Sent"><Icon name="check"/><span class="sr-only">"Sent"</span></span>
        }.into_any(),
        "delivered" | "read" => {
            let read = status == "read";
            let label = if read { "Read" } else { "Delivered" };
            view! {
                <span class=if read { "chat-tick is-read" } else { "chat-tick" } title=label>
                    <Icon name="checks"/><span class="sr-only">{label}</span>
                </span>
            }.into_any()
        }
        _ => ().into_any(),
    }
}

/// Runs a confirmation's work, closing on success and showing the error otherwise.
#[component]
pub fn ConfirmAction(confirm: Confirm, on_close: Callback<()>) -> impl IntoView {
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

// QR code encoder (byte mode, error correction M), ported from `next/shared/qr.ts`.
const ECC_PER_BLOCK: [usize; 41] = [
    0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
];
const BLOCK_COUNT: [usize; 41] = [
    0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23,
    25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49,
];
const FORMAT_BITS_M: u32 = 0;

fn raw_modules(version: usize) -> usize {
    let mut result = (16 * version + 128) * version + 64;
    if version >= 2 {
        let align = version / 7 + 2;
        result -= (25 * align - 10) * align - 55;
        if version >= 7 {
            result -= 36;
        }
    }
    result
}

fn data_codewords(version: usize) -> usize {
    raw_modules(version) / 8 - ECC_PER_BLOCK[version] * BLOCK_COUNT[version]
}

fn gf_multiply(x: u8, y: u8) -> u8 {
    let mut z: u16 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11d);
        z ^= (((y >> i) & 1) as u16) * (x as u16);
    }
    z as u8
}

fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root: u8 = 1;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_multiply(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_multiply(root, 2);
    }
    result
}

fn rs_remainder(data: &[u8], div: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; div.len()];
    for &b in data {
        let factor = b ^ result.remove(0);
        result.push(0);
        for (slot, &coef) in result.iter_mut().zip(div.iter()) {
            *slot ^= gf_multiply(coef, factor);
        }
    }
    result
}

fn bit(value: u32, i: u32) -> bool {
    ((value >> i) & 1) != 0
}

pub fn qr_matrix(text: &str) -> Result<Vec<Vec<bool>>, String> {
    let bytes = text.as_bytes();
    let mut version = 1usize;
    while version <= 40 {
        let count_bits = if version < 10 { 8 } else { 16 };
        if 4 + count_bits + 8 * bytes.len() <= data_codewords(version) * 8 {
            break;
        }
        version += 1;
    }
    if version > 40 {
        return Err("The code is too long for a QR code.".into());
    }
    let mut bits: Vec<u8> = Vec::new();
    let push_bits = |bits: &mut Vec<u8>, value: usize, length: usize| {
        for i in (0..length).rev() {
            bits.push(((value >> i) & 1) as u8);
        }
    };
    push_bits(&mut bits, 4, 4);
    push_bits(&mut bits, bytes.len(), if version < 10 { 8 } else { 16 });
    for &b in bytes {
        push_bits(&mut bits, b as usize, 8);
    }
    let capacity = data_codewords(version) * 8;
    let n = 4.min(capacity - bits.len());
    push_bits(&mut bits, 0, n);
    let n = (8 - bits.len() % 8) % 8;
    push_bits(&mut bits, 0, n);
    let mut pad = 0xecusize;
    while bits.len() < capacity {
        push_bits(&mut bits, pad, 8);
        pad ^= 0xec ^ 0x11;
    }
    let mut data: Vec<u8> = Vec::with_capacity(bits.len() / 8);
    for chunk in bits.chunks(8) {
        data.push(chunk.iter().fold(0u8, |a, &b| (a << 1) | b));
    }

    let blocks = BLOCK_COUNT[version];
    let ecc_length = ECC_PER_BLOCK[version];
    let raw = raw_modules(version) / 8;
    let short = blocks - raw % blocks;
    let short_length = raw / blocks;
    let div = rs_divisor(ecc_length);
    let mut parts: Vec<Vec<u8>> = Vec::with_capacity(blocks);
    let mut k = 0usize;
    for i in 0..blocks {
        let len = short_length - ecc_length + usize::from(i >= short);
        let mut chunk = data[k..k + len].to_vec();
        k += len;
        let ecc = rs_remainder(&chunk, &div);
        if i < short {
            chunk.push(0);
        }
        chunk.extend_from_slice(&ecc);
        parts.push(chunk);
    }
    let mut codewords: Vec<u8> = Vec::new();
    let part_len = parts[0].len();
    for i in 0..part_len {
        for (j, part) in parts.iter().enumerate() {
            if i != short_length - ecc_length || j >= short {
                codewords.push(part[i]);
            }
        }
    }

    let size = version * 4 + 17;
    let mut modules = vec![vec![false; size]; size];
    let mut fixed = vec![vec![false; size]; size];
    let set = |modules: &mut Vec<Vec<bool>>,
               fixed: &mut Vec<Vec<bool>>,
               x: usize,
               y: usize,
               dark: bool| {
        modules[y][x] = dark;
        fixed[y][x] = true;
    };
    for i in 0..size {
        set(&mut modules, &mut fixed, 6, i, i % 2 == 0);
        set(&mut modules, &mut fixed, i, 6, i % 2 == 0);
    }
    for &(cx, cy) in &[
        (3isize, 3isize),
        (size as isize - 4, 3),
        (3, size as isize - 4),
    ] {
        for dy in -4..=4 {
            for dx in -4..=4 {
                let (x, y) = (cx + dx, cy + dy);
                let dist = dx.abs().max(dy.abs());
                if x >= 0 && x < size as isize && y >= 0 && y < size as isize {
                    set(
                        &mut modules,
                        &mut fixed,
                        x as usize,
                        y as usize,
                        dist != 2 && dist != 4,
                    );
                }
            }
        }
    }
    if version > 1 {
        let count = version / 7 + 2;
        let step = if version == 32 {
            26
        } else {
            (version * 4 + 4).div_ceil(count * 2 - 2) * 2
        };
        let mut positions = vec![6usize];
        let mut pos = size - 7;
        while positions.len() < count {
            positions.insert(1, pos);
            pos = pos.saturating_sub(step);
        }
        for i in 0..count {
            for j in 0..count {
                if (i == 0 && j == 0) || (i == 0 && j == count - 1) || (i == count - 1 && j == 0) {
                    continue;
                }
                for dy in -2isize..=2 {
                    for dx in -2isize..=2 {
                        let x = (positions[i] as isize + dx) as usize;
                        let y = (positions[j] as isize + dy) as usize;
                        set(&mut modules, &mut fixed, x, y, dx.abs().max(dy.abs()) != 1);
                    }
                }
            }
        }
    }

    let write_format = |modules: &mut Vec<Vec<bool>>, fixed: &mut Vec<Vec<bool>>, mask: u32| {
        let value = (FORMAT_BITS_M << 3) | mask;
        let mut rem = value;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let f = ((value << 10) | rem) ^ 0x5412;
        for i in 0..=5 {
            set(modules, fixed, 8, i, bit(f, i as u32));
        }
        set(modules, fixed, 8, 7, bit(f, 6));
        set(modules, fixed, 8, 8, bit(f, 7));
        set(modules, fixed, 7, 8, bit(f, 8));
        for i in 9..15 {
            set(modules, fixed, 14 - i, 8, bit(f, i as u32));
        }
        for i in 0..8 {
            set(modules, fixed, size - 1 - i, 8, bit(f, i as u32));
        }
        for i in 8..15 {
            set(modules, fixed, 8, size - 15 + i, bit(f, i as u32));
        }
        set(modules, fixed, 8, size - 8, true);
    };
    write_format(&mut modules, &mut fixed, 0);

    if version >= 7 {
        let mut rem = version as u32;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1f25);
        }
        let v = ((version as u32) << 12) | rem;
        for i in 0..18usize {
            let a = size - 11 + i % 3;
            let b = i / 3;
            set(&mut modules, &mut fixed, a, b, bit(v, i as u32));
            set(&mut modules, &mut fixed, b, a, bit(v, i as u32));
        }
    }

    let mut bit_idx = 0usize;
    let mut right = size as isize - 1;
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        for vert in 0..size {
            for j in 0..2 {
                let x = (right - j) as usize;
                let y = if ((right + 1) & 2) == 0 {
                    size - 1 - vert
                } else {
                    vert
                };
                if !fixed[y][x] && bit_idx < codewords.len() * 8 {
                    modules[y][x] = bit(codewords[bit_idx >> 3] as u32, (7 - (bit_idx & 7)) as u32);
                    bit_idx += 1;
                }
            }
        }
        right -= 2;
    }

    let mask_fn = |mask: u32, x: usize, y: usize| -> bool {
        match mask {
            0 => (x + y) % 2 == 0,
            1 => y % 2 == 0,
            2 => x % 3 == 0,
            3 => (x + y) % 3 == 0,
            4 => (x / 3 + y / 2) % 2 == 0,
            5 => (x * y) % 2 + (x * y) % 3 == 0,
            6 => ((x * y) % 2 + (x * y) % 3) % 2 == 0,
            _ => ((x + y) % 2 + (x * y) % 3) % 2 == 0,
        }
    };
    let apply_mask = |modules: &mut Vec<Vec<bool>>, fixed: &[Vec<bool>], mask: u32| {
        for y in 0..size {
            for x in 0..size {
                if !fixed[y][x] && mask_fn(mask, x, y) {
                    modules[y][x] = !modules[y][x];
                }
            }
        }
    };
    let penalty = |modules: &[Vec<bool>]| -> usize {
        let mut score = 0usize;
        let mut dark = 0usize;
        for a in 0..size {
            let (mut row_run, mut col_run) = (1usize, 1usize);
            for b in 1..size {
                if modules[a][b] == modules[a][b - 1] {
                    row_run += 1;
                    if row_run == 5 {
                        score += 3;
                    } else if row_run > 5 {
                        score += 1;
                    }
                } else {
                    row_run = 1;
                }
                if modules[b][a] == modules[b - 1][a] {
                    col_run += 1;
                    if col_run == 5 {
                        score += 3;
                    } else if col_run > 5 {
                        score += 1;
                    }
                } else {
                    col_run = 1;
                }
            }
        }
        for y in 0..size {
            for x in 0..size {
                if modules[y][x] {
                    dark += 1;
                }
                if x < size - 1
                    && y < size - 1
                    && modules[y][x] == modules[y][x + 1]
                    && modules[y][x] == modules[y + 1][x]
                    && modules[y][x] == modules[y + 1][x + 1]
                {
                    score += 3;
                }
            }
        }
        let total = size * size;
        let balance = (dark as isize * 20 - total as isize * 10).unsigned_abs() / total;
        score + balance * 10
    };

    let mut best = 0u32;
    let mut best_score = usize::MAX;
    for mask in 0..8u32 {
        apply_mask(&mut modules, &fixed, mask);
        write_format(&mut modules, &mut fixed, mask);
        let score = penalty(&modules);
        if score < best_score {
            best = mask;
            best_score = score;
        }
        apply_mask(&mut modules, &fixed, mask);
    }
    apply_mask(&mut modules, &fixed, best);
    write_format(&mut modules, &mut fixed, best);
    Ok(modules)
}

/// An SVG path of the dark modules, offset by a four-module quiet zone (`qrPath`).
pub fn qr_path(matrix: &[Vec<bool>]) -> (String, usize) {
    let mut d = String::new();
    for (y, row) in matrix.iter().enumerate() {
        for (x, &dark) in row.iter().enumerate() {
            if dark {
                let _ = write!(&mut d, "M{} {}h1v1h-1z", x + 4, y + 4);
            }
        }
    }
    (d, matrix.len() + 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_prefs_defaults_and_overrides() {
        assert_eq!(ChatPrefs::from_preferences(None), ChatPrefs::default());

        let mut prefs = Preferences::default();
        prefs.rest.insert("chatDensity".into(), serde_json::json!("compact"));
        prefs.rest.insert("chatSendKey".into(), serde_json::json!("modEnter"));
        prefs.rest.insert("chatFormatMarkdown".into(), serde_json::json!(false));
        prefs.rest.insert("chatShowEmoji".into(), serde_json::json!(false));
        let parsed = ChatPrefs::from_preferences(Some(&prefs));
        assert!(parsed.is_compact());
        assert!(parsed.send_on_mod());
        assert!(!parsed.format_markdown);
        assert!(!parsed.show_emoji);
    }

    #[test]
    fn quick_filter_matches_expected_threads() {
        let unread_group = ChatThread {
            kind: "group".into(),
            unread: 2,
            pinned: false,
            ..Default::default()
        };
        let pinned_dm = ChatThread {
            kind: "dm".into(),
            unread: 0,
            pinned: true,
            ..Default::default()
        };
        assert!(QuickFilter::All.matches(&unread_group));
        assert!(QuickFilter::Unread.matches(&unread_group));
        assert!(QuickFilter::Groups.matches(&unread_group));
        assert!(!QuickFilter::Pinned.matches(&unread_group));

        assert!(QuickFilter::Pinned.matches(&pinned_dm));
        assert!(!QuickFilter::Unread.matches(&pinned_dm));
        assert!(!QuickFilter::Groups.matches(&pinned_dm));
    }
}
