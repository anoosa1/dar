//! Chat (`chat.ts` and `chat-discord.ts`): WhatsApp and Discord accounts linked through the
//! bridge, their chats, messages, attachments and reactions, and the bridge's key store.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};
use spacetimedb::{ProcedureContext, ReducerContext, Table};

use crate::app_settings::app_enabled;
use crate::blobs::{is_blob_id, release_blob, store_blob};
use crate::tables::*;
use crate::{Res, fail, now, row};

type Doc = Map<String, Value>;

const ACCOUNTS: usize = 10;
const NAME: usize = 64;
const TEXT: usize = 65536;
const PAGE: f64 = 50.0;
const MESSAGE_PAGE: f64 = 60.0;
const MESSAGES_PER_ACCOUNT: u32 = 500_000;
const AUTO_DOWNLOAD: f64 = 16.0 * 1024.0 * 1024.0;
const STATES: [&str; 6] = ["linking", "connecting", "connected", "disconnected", "loggedOut", "failed"];
const KINDS: [&str; 11] = ["text", "image", "video", "audio", "voice", "document", "sticker", "location", "contact", "poll", "unsupported"];
pub const READ_OPERATIONS: [&str; 19] = ["access", "overview", "threads", "messages", "account", "media", "target", "chatTarget", "chatInfo", "oldest", "names", "contactSearch", "kvLoad", "kvGet", "accounts", "mediaFind", "servers", "find", "newest"];

fn text(value: Option<&Value>, max: usize, allow_empty: bool) -> Res<String> {
    let Some(s) = value.and_then(Value::as_str) else { return fail("invalidInput") };
    if !allow_empty && s.trim().is_empty() || s.contains('\0') || s.encode_utf16().count() > max {
        return fail("invalidInput");
    }
    Ok(s.to_owned())
}

fn optional(value: Option<&Value>, max: usize) -> Res<String> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        v => text(v, max, true),
    }
}

fn keys(p: &Doc, allowed: &[&str]) -> Res<()> {
    if p.keys().any(|k| !allowed.contains(&k.as_str())) { fail("invalidInput") } else { Ok(()) }
}

fn num(value: Option<&Value>) -> Res<f64> {
    match value.and_then(Value::as_f64) {
        Some(f) if f.is_finite() => Ok(f),
        _ => fail("invalidInput"),
    }
}

fn record(value: &Value) -> Res<&Doc> {
    value.as_object().ok_or_else(|| "invalidInput".to_owned())
}

fn prefix(value: &str) -> (std::ops::Bound<String>, std::ops::Bound<String>) {
    (std::ops::Bound::Included(format!("{value}/")), std::ops::Bound::Excluded(format!("{value}0")))
}

fn after(cursor: &str, value: &str) -> (std::ops::Bound<String>, std::ops::Bound<String>) {
    (std::ops::Bound::Excluded(cursor.to_owned()), std::ops::Bound::Excluded(format!("{value}0")))
}

fn jid(value: Option<&Value>) -> Res<String> {
    let v = text(value, 120, true)?;
    let ok = v.split_once('@').is_some_and(|(user, host)| {
        (1..=80).contains(&user.len()) && user.bytes().all(|b| b.is_ascii_alphanumeric() || b".:_-".contains(&b)) && ["s.whatsapp.net", "g.us", "lid"].contains(&host)
    });
    if ok { Ok(v) } else { fail("invalidInput") }
}

pub fn message_id(value: Option<&Value>) -> Res<String> {
    let v = text(value, 128, true)?;
    if (1..=128).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) { Ok(v) } else { fail("invalidInput") }
}

/// A Discord ID: a server, channel, user or message.
pub fn discord_id(value: Option<&Value>) -> Res<String> {
    let v = text(value, 25, true)?;
    if (1..=25).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit()) { Ok(v) } else { fail("invalidInput") }
}

fn status_rank(status: &str) -> Option<u8> {
    match status {
        "" | "failed" | "pending" => Some(0),
        "sent" => Some(1),
        "delivered" => Some(2),
        "read" => Some(3),
        _ => None,
    }
}

fn valid_key(k: &str) -> bool {
    (1..=600).contains(&k.len()) && k.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// A chat ID on the account's platform: a WhatsApp JID or a Discord channel ID.
fn chat_id(a: &ChatAccount, value: Option<&Value>) -> Res<String> {
    if a.platform == "discord" { discord_id(value) } else { jid(value) }
}

pub fn changed(ctx: &ReducerContext, user_id: &str) {
    let old = ctx.db.chat_changes().user_id().find(user_id.to_owned());
    let next = ChatChange { user_id: user_id.into(), revision: old.as_ref().map_or(0, |o| o.revision).wrapping_add(1) };
    if old.is_some() {
        ctx.db.chat_changes().user_id().update(next);
    } else {
        ctx.db.chat_changes().insert(next);
    }
}

/// Fixed-width keys that sort newest first.
pub fn chat_time_key(at: f64) -> String {
    format!("{:013}", (9_999_999_999_999.0 - at.max(0.0).floor()).max(0.0) as i64)
}

pub fn list_key(account_id: &str, at: f64, id: &str) -> String {
    format!("{account_id}/{}/{id}", chat_time_key(at))
}

pub fn thread_key(thread_id: &str, at: f64, id: &str) -> String {
    format!("{thread_id}/{}/{id}", chat_time_key(at))
}

const TWO_DIGIT_CODES: [&str; 44] = ["20", "27", "30", "31", "32", "33", "34", "36", "39", "40", "41", "43", "44", "45", "46", "47", "48", "49", "51", "52", "53", "54", "55", "56", "57", "58", "60", "61", "62", "63", "64", "65", "66", "81", "82", "84", "86", "90", "91", "92", "93", "94", "95", "98"];

/// The country calling code at the start of an international number.
pub fn calling_code(digits: &str) -> String {
    if digits.starts_with('1') || digits.starts_with('7') {
        return digits[..1].to_owned();
    }
    let two: String = digits.chars().take(2).collect();
    if TWO_DIGIT_CODES.contains(&two.as_str()) { two } else { digits.chars().take(3).collect() }
}

/// The international digits a contact's number stands for (`internationalDigits`).
pub fn international_digits(value: &str, own: &str) -> String {
    let trimmed = value.trim();
    let digits: String = trimmed.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 6 || digits.len() > 18 {
        return String::new();
    }
    if trimmed.starts_with('+') {
        return digits;
    }
    if let Some(rest) = digits.strip_prefix("00") {
        return rest.to_owned();
    }
    let code = if own.is_empty() { String::new() } else { calling_code(own) };
    if digits.starts_with('0') {
        return if code.is_empty() { String::new() } else { format!("{code}{}", digits.trim_start_matches('0')) };
    }
    if digits.len() >= 11 {
        return digits;
    }
    if !code.is_empty() && !digits.starts_with(&code) { format!("{code}{digits}") } else { digits }
}

/// Dar contacts by phone number, from every address book the person can see. Never sent out.
struct ContactNames<'a> {
    ctx: &'a ReducerContext,
    user_id: String,
    own: String,
    numbers: RefCell<Option<HashMap<String, Value>>>,
}

impl<'a> ContactNames<'a> {
    fn new(ctx: &'a ReducerContext, user_id: &str, own_jid: &str) -> Self {
        Self { ctx, user_id: user_id.into(), own: own_jid.split('@').next().unwrap_or_default().into(), numbers: RefCell::new(None) }
    }
    fn build(&self) -> HashMap<String, Value> {
        let mut numbers = HashMap::new();
        let mut scanned = 0;
        for (book, r) in books_rows(self.ctx, &self.user_id) {
            scanned += 1;
            if scanned > 20000 {
                break;
            }
            let Ok(data) = serde_json::from_str::<Value>(&r.projection) else { continue };
            let name = data["displayName"].as_str().map(|s| s.trim().to_owned()).unwrap_or_default();
            let Some(phones) = data["phones"].as_array().filter(|_| !name.is_empty()) else { continue };
            for phone in phones {
                let digits = international_digits(&crate::text_of(Some(phone.get("value").unwrap_or(&json!("")))), &self.own);
                if !digits.is_empty() && !numbers.contains_key(&digits) {
                    numbers.insert(digits, json!({ "name": name, "book": book, "href": r.href }));
                }
            }
        }
        numbers
    }
    fn find(&self, jid: &str) -> Option<Value> {
        if !jid.ends_with("@s.whatsapp.net") {
            return None;
        }
        if self.numbers.borrow().is_none() {
            let built = self.build();
            *self.numbers.borrow_mut() = Some(built);
        }
        self.numbers.borrow().as_ref().and_then(|n| n.get(jid.split('@').next().unwrap_or_default()).cloned())
    }
}

/// The contact rows of every address book the account owns or was granted, in book order.
fn books_rows(ctx: &ReducerContext, user_id: &str) -> Vec<(String, PimResource)> {
    let mut books: Vec<PimCollection> = ctx.db.pim_collections().owner_id().filter(user_id).filter(|c| c.kind == "contacts").collect();
    for grant in ctx.db.pim_grants().user_id().filter(user_id) {
        if let Some(book) = ctx.db.pim_collections().id().find(grant.collection_id).filter(|b| b.kind == "contacts") {
            books.push(book);
        }
    }
    let mut out = Vec::new();
    for book in books {
        for r in ctx.db.pim_resources().collection_id().filter(book.id.as_str()) {
            out.push((book.id.clone(), r));
            if out.len() > 20001 {
                return out;
            }
        }
    }
    out
}

fn expose_thread(r: &ChatThread, names: Option<&ContactNames>) -> Value {
    let contact = if r.kind == "direct" { names.and_then(|n| n.find(&r.jid)) } else { None };
    let sender = if r.preview_sender_jid.is_empty() { None } else { names.and_then(|n| n.find(&r.preview_sender_jid)) };
    let name = contact.as_ref().and_then(|c| c["name"].as_str().map(str::to_owned)).unwrap_or_else(|| r.name.clone());
    let mut v = json!({ "id": r.id, "accountId": r.account_id, "jid": r.jid, "kind": r.kind, "name": name, "whatsappName": r.name, "contact": contact, "at": r.at,
        "preview": r.preview, "previewKind": r.preview_kind, "previewFromMe": r.preview_from_me, "previewSender": r.preview_sender, "previewStatus": r.preview_status,
        "unread": r.unread, "archived": r.archived, "pinned": r.pinned, "mutedUntil": r.muted_until, "serverId": r.server_id, "section": r.section, "order": r.order,
        "topic": r.topic, "mentions": r.mentions });
    if let Some(sender) = sender {
        v["previewSender"] = sender["name"].clone();
    }
    v
}

fn expose_message(r: &ChatMessage, media: Option<&ChatMedia>, reactions: Vec<Value>) -> Value {
    let media = match media {
        Some(m) if !r.deleted => json!({ "kind": m.kind, "mime": m.mime, "size": m.size, "seconds": m.seconds, "width": m.width, "height": m.height, "state": m.state }),
        _ => Value::Null,
    };
    json!({ "id": r.id, "messageId": r.message_id, "sender": r.sender, "senderName": r.sender_name, "fromMe": r.from_me, "at": r.at, "kind": r.kind,
        "text": if r.deleted { String::new() } else { r.text.clone() }, "fileName": r.file_name, "quotedId": r.quoted_id, "quotedSender": r.quoted_sender,
        "quotedText": r.quoted_text, "status": r.status, "edited": r.edited, "deleted": r.deleted, "media": media, "reactions": if r.deleted { Vec::new() } else { reactions } })
}

/// Reactions grouped by emoji, with who reacted and whether the account did.
fn reactions_of(ctx: &ReducerContext, row_id: &str) -> Vec<Value> {
    let mut groups: Vec<(String, u32, bool, Vec<String>)> = Vec::new();
    for r in ctx.db.chat_reactions().message_row_id().filter(row_id) {
        let index = match groups.iter().position(|g| g.0 == r.emoji) {
            Some(i) => i,
            None => {
                groups.push((r.emoji.clone(), 0, false, Vec::new()));
                groups.len() - 1
            }
        };
        let g = &mut groups[index];
        // Discord's "*" rows count everyone else without names.
        g.1 += r.count;
        if r.from_me {
            g.2 = true;
        } else if r.sender != "*" && g.3.len() < 20 {
            g.3.push(if r.sender_name.is_empty() { r.sender.split('@').next().unwrap_or_default().to_owned() } else { r.sender_name.clone() });
        }
    }
    groups.sort_by(|a, b| b.1.cmp(&a.1));
    groups.into_iter().map(|(emoji, count, mine, names)| json!({ "emoji": emoji, "count": count, "mine": mine, "names": names })).collect()
}

/// One reaction per person; a newer change wins and an empty emoji removes it.
fn set_reaction(ctx: &ReducerContext, account_id: &str, row_id: &str, sender: &str, sender_name: &str, from_me: bool, emoji: &str, at: f64) {
    let id = format!("{row_id}|{}", if from_me { "me" } else { sender });
    let old = ctx.db.chat_reactions().id().find(id.clone());
    if old.as_ref().is_some_and(|o| o.at > at) {
        return;
    }
    if emoji.is_empty() {
        if old.is_some() {
            ctx.db.chat_reactions().id().delete(id);
        }
        return;
    }
    let next = ChatReaction { id, message_row_id: row_id.into(), account_id: account_id.into(), sender: sender.into(), sender_name: sender_name.into(), from_me, emoji: emoji.into(), at, count: 1 };
    if old.is_some() {
        ctx.db.chat_reactions().id().update(next);
    } else {
        ctx.db.chat_reactions().insert(next);
    }
}

/// An edit or a deletion for everyone; a deletion takes the attachment and reactions with it.
fn revise_message(ctx: &ReducerContext, thread: &ChatThread, target: &ChatMessage, text: Option<String>, deleted: bool) -> ChatMessage {
    let next = if deleted { ChatMessage { deleted: true, text: String::new(), ..target.clone() } } else { ChatMessage { text: text.unwrap_or(target.text.clone()), edited: true, ..target.clone() } };
    ctx.db.chat_messages().id().update(next.clone());
    if next.deleted {
        if let Some(media) = ctx.db.chat_media().id().find(target.id.clone()) {
            drop_media(ctx, &media);
        }
        for r in ctx.db.chat_reactions().message_row_id().filter(target.id.as_str()).collect::<Vec<_>>() {
            ctx.db.chat_reactions().id().delete(r.id);
        }
    }
    if let Some(current) = ctx.db.chat_threads().id().find(thread.id.clone())
        && current.at == target.at
        && current.preview_from_me == target.from_me
    {
        let updated = ChatThread { preview: if next.deleted { String::new() } else { next.text.clone() }, preview_kind: if next.deleted { "deleted".into() } else { current.preview_kind.clone() }, ..current.clone() };
        set_thread(ctx, &current, updated);
    }
    next
}

fn expose_account(ctx: &ReducerContext, r: &ChatAccount) -> Value {
    let (mut chats, mut unread_chats, mut groups, mut archived, mut unread) = (0, 0, 0, 0, 0);
    for thread in ctx.db.chat_threads().account_id().filter(r.id.as_str()) {
        // Discord server channels count only when they mention the account.
        if !thread.server_id.is_empty() {
            if thread.mentions > 0 {
                unread += 1;
            }
            continue;
        }
        if thread.archived {
            archived += 1;
            continue;
        }
        chats += 1;
        if thread.kind == "group" {
            groups += 1;
        }
        if thread.unread > 0 {
            unread_chats += 1;
            if thread.muted_until == 0.0 || thread.muted_until > 0.0 && thread.muted_until < now(ctx) {
                unread += 1;
            }
        }
    }
    json!({ "id": r.id, "platform": r.platform, "name": r.name, "jid": r.jid, "state": r.state, "detail": r.detail, "createdAt": r.created_at, "linkedAt": r.linked_at,
        "syncedAt": r.synced_at, "historyProgress": r.history_progress, "importing": r.importing, "unread": unread,
        "counts": { "chats": chats, "unread": unread_chats, "groups": groups, "archived": archived } })
}

fn delete_chats(ctx: &ReducerContext, account_id: &str) {
    for r in ctx.db.chat_messages().account_id().filter(account_id).collect::<Vec<_>>() {
        ctx.db.chat_messages().id().delete(r.id);
    }
    for r in ctx.db.chat_threads().account_id().filter(account_id).collect::<Vec<_>>() {
        ctx.db.chat_threads().id().delete(r.id);
    }
    for r in ctx.db.chat_media().account_id().filter(account_id).collect::<Vec<_>>() {
        drop_media(ctx, &r);
    }
    for r in ctx.db.chat_reactions().account_id().filter(account_id).collect::<Vec<_>>() {
        ctx.db.chat_reactions().id().delete(r.id);
    }
    for r in ctx.db.chat_servers().account_id().filter(account_id).collect::<Vec<_>>() {
        ctx.db.chat_servers().id().delete(r.id);
    }
}

/// Maintenance removes the blob once nothing else uses it.
fn drop_media(ctx: &ReducerContext, r: &ChatMedia) {
    ctx.db.chat_media().id().delete(r.id.clone());
    if !r.blob_id.is_empty() {
        release_blob(ctx, &r.blob_id);
    }
}

fn delete_account_rows(ctx: &ReducerContext, account: &ChatAccount) {
    delete_chats(ctx, &account.id);
    clear_keys(ctx, &account.id);
    ctx.db.chat_accounts().id().delete(account.id.clone());
}

fn clear_keys(ctx: &ReducerContext, account_id: &str) {
    let range = prefix(account_id);
    for r in ctx.db.chat_keys().scan_key().filter(crate::bounds(&range)).collect::<Vec<_>>() {
        ctx.db.chat_keys().id().delete(r.id);
    }
}

fn set_thread(ctx: &ReducerContext, old: &ChatThread, next: ChatThread) {
    if old.list_key != next.list_key {
        ctx.db.chat_threads().id().delete(old.id.clone());
        ctx.db.chat_threads().insert(next);
    } else {
        ctx.db.chat_threads().id().update(next);
    }
}

fn new_thread(account: &ChatAccount, id: &str, kind: &str, at: f64) -> ChatThread {
    ChatThread {
        id: format!("{}/{id}", account.id),
        account_id: account.id.clone(),
        list_key: list_key(&account.id, at, id),
        jid: id.into(),
        kind: kind.into(),
        name: String::new(),
        at,
        preview: String::new(),
        preview_kind: String::new(),
        preview_from_me: false,
        preview_sender: String::new(),
        preview_status: String::new(),
        unread: 0,
        archived: false,
        pinned: false,
        muted_until: 0.0,
        preview_sender_jid: String::new(),
        server_id: String::new(),
        section: String::new(),
        order: String::new(),
        topic: String::new(),
        mentions: 0,
    }
}

struct MediaInput {
    mime: String,
    size: f64,
    seconds: u32,
    width: u32,
    height: u32,
}

fn valid_media(value: Option<&Value>) -> Res<Option<MediaInput>> {
    let Some(value) = value.filter(|v| !v.is_null()) else { return Ok(None) };
    let v = record(value)?;
    keys(v, &["mime", "size", "seconds", "width", "height"])?;
    let count = |k: &str, max: f64| -> Res<f64> { if v.contains_key(k) { Ok(num(v.get(k))?.floor().clamp(0.0, max)) } else { Ok(0.0) } };
    let mime = optional(v.get("mime"), 150)?.to_lowercase();
    // An odd type never blocks the message; the file is then only offered as a download.
    let token = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".+-".contains(&b));
    let mime = if mime.split_once('/').is_some_and(|(a, b)| token(a) && token(b)) { mime } else { "application/octet-stream".into() };
    Ok(Some(MediaInput { mime, size: count("size", 9_007_199_254_740_991.0)?, seconds: count("seconds", 1e7)? as u32, width: count("width", 1e5)? as u32, height: count("height", 1e5)? as u32 }))
}

struct ChatInput {
    id: String,
    kind: String,
    name: String,
    unread: Option<u32>,
    archived: Option<bool>,
    pinned: Option<bool>,
    muted_until: Option<f64>,
    at: f64,
}

fn flag(v: Option<&Value>) -> Res<Option<bool>> {
    match v {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        _ => fail("invalidInput"),
    }
}

fn valid_chat(value: &Value) -> Res<ChatInput> {
    let c = record(value)?;
    keys(c, &["id", "kind", "name", "unread", "archived", "pinned", "mutedUntil", "at"])?;
    let kind = match c.get("kind").and_then(Value::as_str) {
        Some("group") => "group",
        Some("direct") => "direct",
        _ => return fail("invalidInput"),
    };
    Ok(ChatInput {
        id: jid(c.get("id"))?,
        kind: kind.into(),
        name: optional(c.get("name"), 200)?.trim().to_owned(),
        unread: if c.contains_key("unread") { Some(num(c.get("unread"))?.floor().clamp(0.0, 1_000_000.0) as u32) } else { None },
        archived: flag(c.get("archived"))?,
        pinned: flag(c.get("pinned"))?,
        muted_until: if c.contains_key("mutedUntil") { Some(num(c.get("mutedUntil"))?) } else { None },
        at: if c.contains_key("at") { num(c.get("at"))?.max(0.0) } else { 0.0 },
    })
}

struct MessageInput {
    id: String,
    chat: String,
    sender: String,
    sender_name: String,
    from_me: bool,
    at: f64,
    kind: String,
    text: String,
    file_name: String,
    target: String,
    quoted_id: String,
    quoted_sender: String,
    quoted_text: String,
    status: String,
    media: Option<MediaInput>,
    nonce: String,
    mention: bool,
    reactions: Option<Vec<(String, u32, bool)>>,
}

fn valid_message(value: &Value) -> Res<MessageInput> {
    let m = record(value)?;
    keys(m, &["id", "chat", "sender", "senderName", "fromMe", "at", "kind", "text", "fileName", "target", "quotedId", "quotedSender", "quotedText", "status", "media"])?;
    let kind = text(m.get("kind"), 20, true)?;
    if !KINDS.contains(&kind.as_str()) && !["edit", "revoke", "reaction"].contains(&kind.as_str()) {
        return fail("invalidInput");
    }
    let status = optional(m.get("status"), 20)?;
    if status_rank(&status).is_none() {
        return fail("invalidInput");
    }
    let Some(from_me) = m.get("fromMe").and_then(Value::as_bool) else { return fail("invalidInput") };
    Ok(MessageInput {
        id: message_id(m.get("id"))?,
        chat: jid(m.get("chat"))?,
        sender: optional(m.get("sender"), 120)?,
        sender_name: optional(m.get("senderName"), 200)?,
        from_me,
        at: num(m.get("at"))?.max(0.0),
        kind,
        text: optional(m.get("text"), TEXT)?,
        file_name: optional(m.get("fileName"), 500)?,
        target: if m.contains_key("target") { message_id(m.get("target"))? } else { String::new() },
        quoted_id: if m.contains_key("quotedId") { message_id(m.get("quotedId"))? } else { String::new() },
        quoted_sender: optional(m.get("quotedSender"), 200)?,
        quoted_text: optional(m.get("quotedText"), 2000)?,
        status,
        media: valid_media(m.get("media"))?,
        nonce: String::new(),
        mention: false,
        reactions: None,
    })
}

fn insert_message(ctx: &ReducerContext, account: &ChatAccount, thread: &ChatThread, m: &MessageInput, status: String) -> String {
    let id = format!("{}/{}", thread.id, m.id);
    ctx.db.chat_messages().insert(ChatMessage {
        id: id.clone(),
        account_id: account.id.clone(),
        thread_key: thread_key(&thread.id, m.at, &m.id),
        message_id: m.id.clone(),
        sender: m.sender.clone(),
        sender_name: m.sender_name.clone(),
        from_me: m.from_me,
        at: m.at,
        kind: m.kind.clone(),
        text: m.text.clone(),
        file_name: m.file_name.clone(),
        quoted_id: m.quoted_id.clone(),
        quoted_sender: m.quoted_sender.clone(),
        quoted_text: m.quoted_text.clone(),
        status,
        edited: false,
        deleted: false,
    });
    id
}

fn insert_media(ctx: &ReducerContext, account: &ChatAccount, id: &str, m: &MessageInput, media: &MediaInput) {
    ctx.db.chat_media().insert(ChatMedia {
        id: id.into(),
        account_id: account.id.clone(),
        blob_id: String::new(),
        kind: m.kind.clone(),
        mime: media.mime.clone(),
        size: media.size,
        file_name: m.file_name.clone(),
        seconds: media.seconds,
        width: media.width,
        height: media.height,
        state: "remote".into(),
        at: m.at,
    });
}

/// Stores chats and messages from the bridge; history keeps the phone's unread counts, live
/// messages add to them.
fn ingest(ctx: &ReducerContext, account: &ChatAccount, p: &Doc) -> Res<Value> {
    let kind = text(p.get("kind"), 20, true)?;
    let live = kind == "messages";
    let chats = p.get("chats").and_then(Value::as_array).map(|a| a.iter().map(valid_chat).collect::<Res<Vec<_>>>()).transpose()?.unwrap_or_default();
    let mut messages = p.get("messages").and_then(Value::as_array).map(|a| a.iter().map(valid_message).collect::<Res<Vec<_>>>()).transpose()?.unwrap_or_default();
    if chats.len() > 5000 || messages.len() > 2000 {
        return fail("operationTooLarge");
    }
    let mut stored = account.messages;
    let mut download: Vec<String> = Vec::new();
    let thread = |id: &str, kind_of: &str, create: bool| -> Option<ChatThread> {
        let found = ctx.db.chat_threads().id().find(format!("{}/{id}", account.id));
        if found.is_some() || !create {
            return found;
        }
        let created = new_thread(account, id, kind_of, 0.0);
        ctx.db.chat_threads().insert(created.clone());
        Some(created)
    };
    for c in &chats {
        let Some(old) = thread(&c.id, &c.kind, kind != "chatState") else { continue };
        let mut next = ChatThread { kind: c.kind.clone(), ..old.clone() };
        if !c.name.is_empty() {
            next.name = c.name.clone();
        }
        if let Some(u) = c.unread {
            next.unread = u;
        }
        if let Some(a) = c.archived {
            next.archived = a;
        }
        if let Some(pinned) = c.pinned {
            next.pinned = pinned;
        }
        if let Some(m) = c.muted_until {
            next.muted_until = m;
        }
        if c.at > next.at {
            next.at = c.at;
            next.list_key = list_key(&account.id, c.at, &c.id);
        }
        set_thread(ctx, &old, next);
    }
    // Oldest first, so the newest message ends up as the preview.
    messages.sort_by(|a, b| a.at.total_cmp(&b.at));
    for m in &messages {
        if m.kind == "reaction" {
            // Kept even before the message itself arrives (history can bring it later).
            if let Some(t) = thread(&m.chat, "direct", false)
                && !m.target.is_empty()
            {
                let emoji: String = m.text.encode_utf16().take(32).collect::<Vec<u16>>().pipe_utf16();
                set_reaction(ctx, &account.id, &format!("{}/{}", t.id, m.target), &m.sender, &m.sender_name, m.from_me, &emoji, m.at);
            }
            continue;
        }
        let t = thread(&m.chat, if m.chat.ends_with("@g.us") { "group" } else { "direct" }, true).expect("thread");
        if m.kind == "edit" || m.kind == "revoke" {
            if let Some(target) = ctx.db.chat_messages().id().find(format!("{}/{}", t.id, m.target)) {
                revise_message(ctx, &t, &target, (m.kind == "edit").then(|| m.text.clone()), m.kind == "revoke");
            }
            continue;
        }
        let id = format!("{}/{}", t.id, m.id);
        if let Some(old) = ctx.db.chat_messages().id().find(id.clone()) {
            // History repeats live messages; only a better delivery status changes them.
            if old.from_me && status_rank(&m.status) > status_rank(&old.status) {
                ctx.db.chat_messages().id().update(ChatMessage { status: m.status.clone(), ..old });
            }
            continue;
        }
        if stored >= MESSAGES_PER_ACCOUNT {
            break;
        }
        insert_message(ctx, account, &t, m, if m.from_me { if m.status.is_empty() { "sent".into() } else { m.status.clone() } } else { String::new() });
        stored += 1;
        if let Some(media) = &m.media {
            insert_media(ctx, account, &id, m, media);
            // New photos, stickers and voice notes download right away; the rest when opened.
            if live && ["image", "sticker", "voice"].contains(&m.kind.as_str()) && media.size <= AUTO_DOWNLOAD {
                download.push(id.clone());
            }
        }
        let mut next = t.clone();
        if m.at >= t.at {
            next.at = m.at;
            next.list_key = list_key(&account.id, m.at, &t.jid);
            next.preview = if m.text.is_empty() { m.file_name.clone() } else { m.text.clone() };
            next.preview_kind = m.kind.clone();
            next.preview_from_me = m.from_me;
            next.preview_sender = if t.kind == "group" && !m.from_me { m.sender_name.clone() } else { String::new() };
            next.preview_sender_jid = if t.kind == "group" && !m.from_me { m.sender.clone() } else { String::new() };
            next.preview_status = if m.from_me { if m.status.is_empty() { "sent".into() } else { m.status.clone() } } else { String::new() };
        }
        if live {
            next.unread = if m.from_me { 0 } else { (t.unread + 1).min(1_000_000) };
        }
        set_thread(ctx, &t, next);
    }
    if kind == "receipts" {
        let chat = jid(p.get("chat"))?;
        let status = text(p.get("status"), 20, true)?;
        let Some(ids) = p.get("ids").and_then(Value::as_array) else { return fail("invalidInput") };
        let ids = ids.iter().map(|v| message_id(Some(v))).collect::<Res<Vec<_>>>()?;
        if let Some(t) = thread(&chat, "direct", false)
            && ids.len() <= 1000
        {
            if p.get("fromMe") == Some(&json!(true)) && (status == "read" || status == "readSelf") {
                if t.unread > 0 {
                    set_thread(ctx, &t, ChatThread { unread: 0, ..t.clone() });
                }
            } else if p.get("fromMe") == Some(&json!(false)) && status_rank(&status).is_some() {
                for id in ids {
                    if let Some(r) = ctx.db.chat_messages().id().find(format!("{}/{id}", t.id))
                        && r.from_me
                        && status_rank(&status) > status_rank(&r.status)
                    {
                        let at = r.at;
                        ctx.db.chat_messages().id().update(ChatMessage { status: status.clone(), ..r });
                        if let Some(current) = ctx.db.chat_threads().id().find(t.id.clone())
                            && current.preview_from_me
                            && current.at == at
                        {
                            set_thread(ctx, &current, ChatThread { preview_status: status.clone(), ..current.clone() });
                        }
                    }
                }
            }
        }
    }
    let is_final = kind == "history" && p.get("final") == Some(&json!(true));
    let progress = p.get("progress").and_then(Value::as_f64);
    let importing = if kind == "history" { !is_final || progress.unwrap_or(100.0) < 100.0 } else { account.importing };
    let history_progress = if is_final && let Some(pr) = progress { pr.floor().clamp(0.0, 100.0) as u32 } else { account.history_progress };
    let current = ctx.db.chat_accounts().id().find(account.id.clone()).unwrap_or_else(|| account.clone());
    ctx.db.chat_accounts().id().update(ChatAccount { messages: stored, synced_at: now(ctx), importing, history_progress, ..current });
    changed(ctx, &account.owner_id);
    Ok(json!({ "ok": true, "download": download }))
}

trait Utf16 {
    fn pipe_utf16(self) -> String;
}
impl Utf16 for Vec<u16> {
    fn pipe_utf16(self) -> String {
        String::from_utf16_lossy(&self)
    }
}

/// Pages a chat list newest first; pinned chats lead the first page of Chats.
fn thread_page(ctx: &ReducerContext, account: &ChatAccount, view: &str, cursor: &str, limit: usize, names: &ContactNames) -> Value {
    let matches = |r: &ChatThread| -> bool {
        if !r.server_id.is_empty() {
            return false;
        }
        match view {
            "archived" => r.archived,
            "chats" => !r.archived && !r.pinned,
            "unread" => !r.archived && r.unread > 0,
            _ => !r.archived && r.kind == "group",
        }
    };
    let pinned: Vec<Value> = if view == "chats" && cursor.is_empty() {
        let mut list: Vec<ChatThread> = ctx.db.chat_threads().account_id().filter(account.id.as_str()).filter(|r| r.pinned && !r.archived && r.server_id.is_empty()).collect();
        list.sort_by(|a, b| b.at.total_cmp(&a.at));
        list.iter().map(|r| expose_thread(r, Some(names))).collect()
    } else {
        Vec::new()
    };
    let range = if cursor.is_empty() { prefix(&account.id) } else { after(cursor, &account.id) };
    let mut threads: Vec<ChatThread> = Vec::new();
    let mut next = Value::Null;
    for r in ctx.db.chat_threads().list_key().filter(crate::bounds(&range)) {
        if !matches(&r) {
            continue;
        }
        if threads.len() == limit {
            next = json!(threads.last().map(|t| t.list_key.clone()));
            break;
        }
        threads.push(r);
    }
    json!({ "pinned": pinned, "threads": threads.iter().map(|r| expose_thread(r, Some(names))).collect::<Vec<_>>(), "cursor": next })
}

fn message_page(ctx: &ReducerContext, thread: &ChatThread, before: &str, limit: usize, names: &ContactNames) -> Value {
    let range = if before.is_empty() { prefix(&thread.id) } else { after(before, &thread.id) };
    let mut rows: Vec<ChatMessage> = Vec::new();
    let mut more = Value::Null;
    for r in ctx.db.chat_messages().thread_key().filter(crate::bounds(&range)) {
        if rows.len() == limit {
            more = json!(rows.last().map(|m| m.thread_key.clone()));
            break;
        }
        rows.push(r);
    }
    rows.reverse();
    // A Dar contact names group senders too.
    let messages: Vec<Value> = rows
        .iter()
        .map(|r| {
            let contact = if thread.kind == "group" && !r.from_me { names.find(&r.sender) } else { None };
            let named = match contact {
                Some(c) => ChatMessage { sender_name: c["name"].as_str().unwrap_or_default().into(), ..r.clone() },
                None => r.clone(),
            };
            expose_message(&named, ctx.db.chat_media().id().find(r.id.clone()).as_ref(), reactions_of(ctx, &r.id))
        })
        .collect();
    json!({ "thread": expose_thread(thread, Some(names)), "messages": messages, "before": more })
}

fn first_message(ctx: &ReducerContext, thread_id: &str) -> Option<ChatMessage> {
    let range = prefix(thread_id);
    ctx.db.chat_messages().thread_key().filter(crate::bounds(&range)).next()
}

/// Account-scoped Chat operations for browser sessions.
pub fn chat_operation(ctx: &ReducerContext, user_id: &str, operation: &str, p: &Doc) -> Res<Value> {
    if !app_enabled(ctx, "chat") {
        return fail("notFound");
    }
    let account = |value: Option<&Value>| -> Res<ChatAccount> {
        let id = text(value, 64, true)?;
        ctx.db.chat_accounts().id().find(id).filter(|a| a.owner_id == user_id).ok_or_else(|| "notFound".to_owned())
    };
    let thread = |value: Option<&Value>| -> Res<ChatThread> {
        let id = text(value, 300, true)?;
        let Some(r) = ctx.db.chat_threads().id().find(id) else { return fail("notFound") };
        account(Some(&json!(r.account_id)))?;
        Ok(r)
    };
    let limit = |value: Option<&Value>, max: f64| -> Res<usize> { if value.is_none() { Ok(max as usize) } else { Ok(num(value)?.floor().clamp(1.0, max) as usize) } };
    let message = |t: &ChatThread, value: Option<&Value>| -> Res<ChatMessage> { ctx.db.chat_messages().id().find(format!("{}/{}", t.id, message_id(value)?)).ok_or_else(|| "notFound".to_owned()) };
    match operation {
        "access" => {
            keys(p, &[])?;
            Ok(json!({ "ok": true }))
        }
        "overview" => {
            keys(p, &[])?;
            let mut list: Vec<ChatAccount> = ctx.db.chat_accounts().owner_id().filter(user_id).collect();
            list.sort_by(|a, b| a.created_at.total_cmp(&b.created_at));
            Ok(json!({ "accounts": list.iter().map(|a| expose_account(ctx, a)).collect::<Vec<_>>() }))
        }
        "threads" => {
            keys(p, &["accountId", "view", "cursor", "limit"])?;
            let view = crate::text_of(p.get("view"));
            if !["chats", "unread", "groups", "archived"].contains(&view.as_str()) {
                return fail("invalidInput");
            }
            let a = account(p.get("accountId"))?;
            let cursor = optional(p.get("cursor"), 400)?;
            if !cursor.is_empty() && !cursor.starts_with(&format!("{}/", a.id)) {
                return fail("invalidInput");
            }
            let names = ContactNames::new(ctx, user_id, &a.jid);
            Ok(thread_page(ctx, &a, &view, &cursor, limit(p.get("limit"), PAGE)?, &names))
        }
        "messages" => {
            keys(p, &["threadId", "before", "limit"])?;
            let t = thread(p.get("threadId"))?;
            let before = optional(p.get("before"), 500)?;
            if !before.is_empty() && !before.starts_with(&format!("{}/", t.id)) {
                return fail("invalidInput");
            }
            let a = account(Some(&json!(t.account_id)))?;
            let names = ContactNames::new(ctx, user_id, &a.jid);
            Ok(message_page(ctx, &t, &before, limit(p.get("limit"), MESSAGE_PAGE)?, &names))
        }
        "markRead" => {
            // Returns the newest incoming messages so the runtime can send read receipts.
            keys(p, &["threadId"])?;
            let t = thread(p.get("threadId"))?;
            let a = account(Some(&json!(t.account_id)))?;
            let mut items = Vec::new();
            let mut last = String::new();
            if t.unread > 0 || t.mentions > 0 {
                let range = prefix(&t.id);
                for r in ctx.db.chat_messages().thread_key().filter(crate::bounds(&range)) {
                    if last.is_empty() {
                        last = r.message_id.clone();
                    }
                    if items.len() >= (t.unread.min(50) as usize) || a.platform == "discord" {
                        break;
                    }
                    if !r.from_me && !r.deleted {
                        items.push(json!({ "id": r.message_id, "sender": r.sender }));
                    }
                }
                ctx.db.chat_threads().id().update(ChatThread { unread: 0, mentions: 0, ..t.clone() });
                changed(ctx, user_id);
            }
            Ok(json!({ "accountId": t.account_id, "platform": a.platform, "chat": t.jid, "items": if a.platform == "discord" { Vec::new() } else { items }, "last": last }))
        }
        "beginLink" => {
            keys(p, &["accountId", "name", "platform"])?;
            let platform = match p.get("platform") {
                None => "whatsapp",
                Some(v) if v == "whatsapp" => "whatsapp",
                Some(v) if v == "discord" => "discord",
                _ => return fail("invalidInput"),
            };
            let label = if platform == "discord" { "Discord" } else { "WhatsApp" };
            let name = text(Some(p.get("name").unwrap_or(&json!(label))), NAME, true)?.trim().to_owned();
            let name = if name.is_empty() { label.to_owned() } else { name };
            if p.contains_key("accountId") {
                // Linking again keeps the account's history but starts with new device keys.
                let a = account(p.get("accountId"))?;
                if a.state != "loggedOut" && a.state != "failed" && a.state != "linking" || p.contains_key("platform") && p.get("platform").and_then(Value::as_str) != Some(a.platform.as_str()) {
                    return fail("conflict");
                }
                clear_keys(ctx, &a.id);
                let id = a.id.clone();
                ctx.db.chat_accounts().id().update(ChatAccount { state: "linking".into(), detail: String::new(), ..a });
                changed(ctx, user_id);
                return Ok(json!({ "accountId": id }));
            }
            let mine: Vec<ChatAccount> = ctx.db.chat_accounts().owner_id().filter(user_id).collect();
            if mine.len() >= ACCOUNTS {
                return fail("quotaExceeded");
            }
            // An abandoned link attempt is replaced instead of piling up.
            for old in &mine {
                if old.state == "linking" && old.jid.is_empty() {
                    delete_account_rows(ctx, old);
                }
            }
            let mut id = ctx.new_uuid_v4().expect("randomness").to_string();
            while ctx.db.chat_accounts().id().find(id.clone()).is_some() {
                id = ctx.new_uuid_v4().expect("randomness").to_string();
            }
            ctx.db.chat_accounts().insert(ChatAccount { id: id.clone(), owner_id: user_id.into(), platform: platform.into(), name, jid: String::new(), state: "linking".into(), detail: String::new(),
                created_at: now(ctx), linked_at: 0.0, synced_at: 0.0, history_progress: 0, importing: false, messages: 0 });
            changed(ctx, user_id);
            Ok(json!({ "accountId": id }))
        }
        "rename" => {
            keys(p, &["accountId", "name"])?;
            let a = account(p.get("accountId"))?;
            let name = text(p.get("name"), NAME, false)?.trim().to_owned();
            let next = ChatAccount { name, ..a };
            ctx.db.chat_accounts().id().update(next.clone());
            changed(ctx, user_id);
            Ok(expose_account(ctx, &next))
        }
        "account" => {
            keys(p, &["accountId"])?;
            let a = account(p.get("accountId"))?;
            Ok(json!({ "id": a.id, "platform": a.platform, "state": a.state, "jid": a.jid }))
        }
        "servers" => {
            keys(p, &["accountId"])?;
            let a = account(p.get("accountId"))?;
            if a.platform != "discord" {
                return Ok(json!({ "servers": [] }));
            }
            Ok(json!({ "servers": discord_servers(ctx, &a) }))
        }
        "find" => {
            // A chat by its platform ID, after the bridge opened it.
            keys(p, &["accountId", "chat"])?;
            let a = account(p.get("accountId"))?;
            let Some(r) = ctx.db.chat_threads().id().find(format!("{}/{}", a.id, chat_id(&a, p.get("chat"))?)) else { return fail("notFound") };
            Ok(expose_thread(&r, Some(&ContactNames::new(ctx, user_id, &a.jid))))
        }
        "deleteAccount" => {
            keys(p, &["accountId"])?;
            delete_account_rows(ctx, &account(p.get("accountId"))?);
            changed(ctx, user_id);
            Ok(json!({ "ok": true }))
        }
        "openDirect" => {
            // A one-to-one chat by phone number, created if it is not in the list yet.
            keys(p, &["accountId", "phone"])?;
            let a = account(p.get("accountId"))?;
            if a.platform != "whatsapp" {
                return fail("invalidInput");
            }
            let digits: String = text(p.get("phone"), 40, true)?.chars().filter(char::is_ascii_digit).collect();
            if digits.len() < 7 || digits.len() > 15 || digits.starts_with('0') {
                return fail("invalidInput");
            }
            let chat = format!("{digits}@s.whatsapp.net");
            let names = ContactNames::new(ctx, user_id, &a.jid);
            if let Some(old) = ctx.db.chat_threads().id().find(format!("{}/{chat}", a.id)) {
                return Ok(expose_thread(&old, Some(&names)));
            }
            let created = ChatThread { name: format!("+{digits}"), ..new_thread(&a, &chat, "direct", now(ctx)) };
            ctx.db.chat_threads().insert(created.clone());
            changed(ctx, user_id);
            Ok(expose_thread(&created, Some(&names)))
        }
        "prepareSend" => {
            // Stores the outgoing message as pending under the ID the bridge will send it with.
            keys(p, &["threadId", "messageId", "text", "quotedId"])?;
            let t = thread(p.get("threadId"))?;
            let a = account(Some(&json!(t.account_id)))?;
            if !["connected", "connecting", "disconnected"].contains(&a.state.as_str()) {
                return fail("conflict");
            }
            let body = text(p.get("text"), TEXT, false)?;
            let id = message_id(p.get("messageId"))?;
            let at = now(ctx);
            let quoted = match p.get("quotedId") {
                None => None,
                Some(v) if v.as_str() == Some("") => None,
                v => Some(ctx.db.chat_messages().id().find(format!("{}/{}", t.id, message_id(v)?)).ok_or_else(|| "notFound".to_owned())?),
            };
            if ctx.db.chat_messages().id().find(format!("{}/{id}", t.id)).is_some() {
                return fail("alreadyExists");
            }
            let created = ChatMessage {
                id: format!("{}/{id}", t.id),
                account_id: a.id.clone(),
                thread_key: thread_key(&t.id, at, &id),
                message_id: id.clone(),
                sender: a.jid.clone(),
                sender_name: String::new(),
                from_me: true,
                at,
                kind: "text".into(),
                text: body.clone(),
                file_name: String::new(),
                quoted_id: quoted.as_ref().map(|q| q.message_id.clone()).unwrap_or_default(),
                quoted_sender: quoted.as_ref().map(|q| if q.from_me { "You".to_owned() } else if q.sender_name.is_empty() { t.name.clone() } else { q.sender_name.clone() }).unwrap_or_default(),
                quoted_text: quoted.as_ref().map(|q| if q.text.is_empty() { q.file_name.clone() } else { q.text.clone() }).unwrap_or_default(),
                status: "pending".into(),
                edited: false,
                deleted: false,
            };
            ctx.db.chat_messages().insert(created.clone());
            set_thread(ctx, &t, ChatThread { at, list_key: list_key(&a.id, at, &t.jid), preview: body, preview_kind: "text".into(), preview_from_me: true, preview_sender: String::new(), preview_sender_jid: String::new(), preview_status: "pending".into(), unread: 0, ..t.clone() });
            ctx.db.chat_accounts().id().update(ChatAccount { messages: a.messages + 1, ..a.clone() });
            changed(ctx, user_id);
            let quoted = quoted.map(|q| json!({ "id": q.message_id, "sender": if q.from_me { a.jid.clone() } else { q.sender.clone() }, "text": q.text })).unwrap_or(Value::Null);
            Ok(json!({ "accountId": a.id, "platform": a.platform, "chat": t.jid, "message": expose_message(&created, None, Vec::new()), "quoted": quoted }))
        }
        "sendResult" => {
            // Discord gives a sent message its own ID: the pending copy moves there, unless the gateway already did that.
            keys(p, &["threadId", "messageId", "ok", "id"])?;
            let t = thread(p.get("threadId"))?;
            let real_id = if p.contains_key("id") { message_id(p.get("id"))? } else { String::new() };
            let pending = ctx.db.chat_messages().id().find(format!("{}/{}", t.id, message_id(p.get("messageId"))?));
            if pending.is_none()
                && !real_id.is_empty()
                && let Some(sent) = ctx.db.chat_messages().id().find(format!("{}/{real_id}", t.id))
            {
                return Ok(expose_message(&sent, None, Vec::new()));
            }
            let Some(r) = pending else { return fail("notFound") };
            let ok = p.get("ok") == Some(&json!(true));
            let status = if ok { "sent" } else { "failed" };
            if r.status != "pending" {
                return Ok(expose_message(&r, None, Vec::new()));
            }
            let mut next = ChatMessage { status: status.into(), ..r.clone() };
            if ok && !real_id.is_empty() && real_id != r.message_id {
                ctx.db.chat_messages().id().delete(r.id.clone());
                next = ChatMessage { id: format!("{}/{real_id}", t.id), message_id: real_id.clone(), thread_key: thread_key(&t.id, r.at, &real_id), ..next };
                if let Some(existing) = ctx.db.chat_messages().id().find(next.id.clone()) {
                    let a = account(Some(&json!(t.account_id)))?;
                    ctx.db.chat_accounts().id().update(ChatAccount { messages: a.messages.saturating_sub(1), ..a });
                    changed(ctx, user_id);
                    return Ok(expose_message(&existing, None, Vec::new()));
                }
                ctx.db.chat_messages().insert(next.clone());
            } else {
                ctx.db.chat_messages().id().update(next.clone());
            }
            if t.preview_from_me && t.at == r.at {
                ctx.db.chat_threads().id().update(ChatThread { preview_status: status.into(), ..t.clone() });
            }
            changed(ctx, user_id);
            Ok(expose_message(&next, None, Vec::new()))
        }
        "retry" => {
            keys(p, &["threadId", "messageId"])?;
            let t = thread(p.get("threadId"))?;
            let r = ctx.db.chat_messages().id().find(format!("{}/{}", t.id, message_id(p.get("messageId"))?));
            let Some(r) = r.filter(|r| r.from_me && r.status == "failed") else { return fail("conflict") };
            let next = ChatMessage { status: "pending".into(), ..r };
            ctx.db.chat_messages().id().update(next.clone());
            changed(ctx, user_id);
            let quoted = if next.quoted_id.is_empty() { Value::Null } else { json!({ "id": next.quoted_id, "sender": "", "text": next.quoted_text }) };
            Ok(json!({ "accountId": t.account_id, "platform": account(Some(&json!(t.account_id)))?.platform, "chat": t.jid, "message": expose_message(&next, None, Vec::new()), "quoted": quoted }))
        }
        "media" => {
            // What the runtime needs to serve, fetch or save one attachment.
            keys(p, &["threadId", "messageId"])?;
            let t = thread(p.get("threadId"))?;
            let r = ctx.db.chat_messages().id().find(format!("{}/{}", t.id, message_id(p.get("messageId"))?));
            let media = r.as_ref().filter(|r| !r.deleted).and_then(|r| ctx.db.chat_media().id().find(r.id.clone()));
            let (Some(r), Some(media)) = (r, media) else { return fail("notFound") };
            Ok(json!({ "id": media.id, "accountId": t.account_id, "chat": t.jid, "messageId": r.message_id, "kind": media.kind, "mime": media.mime, "size": media.size,
                "fileName": media.file_name, "blobId": media.blob_id, "state": media.state, "at": media.at, "chatName": t.name, "platform": account(Some(&json!(t.account_id)))?.platform }))
        }
        "target" => {
            // What the bridge needs to act on a message: its chat and who sent it.
            keys(p, &["threadId", "messageId"])?;
            let t = thread(p.get("threadId"))?;
            let a = account(Some(&json!(t.account_id)))?;
            let r = message(&t, p.get("messageId"))?;
            Ok(json!({ "accountId": a.id, "platform": a.platform, "state": a.state, "chat": t.jid, "id": r.message_id, "sender": if r.from_me { a.jid.clone() } else { r.sender.clone() },
                "fromMe": r.from_me, "kind": r.kind, "at": r.at, "deleted": r.deleted }))
        }
        "applyDiscordReaction" => {
            keys(p, &["threadId", "messageId", "emoji", "remove"])?;
            let t = thread(p.get("threadId"))?;
            let a = account(Some(&json!(t.account_id)))?;
            let r = message(&t, p.get("messageId"))?;
            if a.platform != "discord" {
                return fail("invalidInput");
            }
            let emoji = text(p.get("emoji"), 120, false)?;
            set_discord_reaction(ctx, &a.id, &r.id, &emoji, "me", if p.get("remove") == Some(&json!(true)) { 0.0 } else { 1.0 }, now(ctx));
            changed(ctx, user_id);
            Ok(json!({ "ok": true }))
        }
        "applyReaction" => {
            keys(p, &["threadId", "messageId", "emoji"])?;
            let t = thread(p.get("threadId"))?;
            let a = account(Some(&json!(t.account_id)))?;
            let r = message(&t, p.get("messageId"))?;
            set_reaction(ctx, &a.id, &r.id, &a.jid, "", true, &text(p.get("emoji"), 32, true)?, now(ctx));
            changed(ctx, user_id);
            Ok(json!({ "ok": true }))
        }
        "applyEdit" => {
            keys(p, &["threadId", "messageId", "text"])?;
            let t = thread(p.get("threadId"))?;
            let r = message(&t, p.get("messageId"))?;
            if !r.from_me || r.deleted || r.kind != "text" {
                return fail("conflict");
            }
            revise_message(ctx, &t, &r, Some(text(p.get("text"), TEXT, false)?), false);
            changed(ctx, user_id);
            Ok(json!({ "ok": true }))
        }
        "applyRevoke" => {
            keys(p, &["threadId", "messageId"])?;
            let t = thread(p.get("threadId"))?;
            let r = message(&t, p.get("messageId"))?;
            if !r.from_me {
                return fail("conflict");
            }
            revise_message(ctx, &t, &r, None, true);
            changed(ctx, user_id);
            Ok(json!({ "ok": true }))
        }
        "deleteForMe" => {
            // Only from Dar; WhatsApp and the phone keep it.
            keys(p, &["threadId", "messageId"])?;
            let t = thread(p.get("threadId"))?;
            let r = message(&t, p.get("messageId"))?;
            if let Some(media) = ctx.db.chat_media().id().find(r.id.clone()) {
                drop_media(ctx, &media);
            }
            for x in ctx.db.chat_reactions().message_row_id().filter(r.id.as_str()).collect::<Vec<_>>() {
                ctx.db.chat_reactions().id().delete(x.id);
            }
            ctx.db.chat_messages().id().delete(r.id.clone());
            if t.at == r.at {
                let next = match first_message(ctx, &t.id) {
                    Some(latest) => ChatThread {
                        preview: if latest.deleted { String::new() } else if latest.text.is_empty() { latest.file_name.clone() } else { latest.text.clone() },
                        preview_kind: if latest.deleted { "deleted".into() } else { latest.kind.clone() },
                        preview_from_me: latest.from_me,
                        preview_sender: if t.kind == "group" && !latest.from_me { latest.sender_name.clone() } else { String::new() },
                        preview_sender_jid: if t.kind == "group" && !latest.from_me { latest.sender.clone() } else { String::new() },
                        preview_status: if latest.from_me { latest.status.clone() } else { String::new() },
                        ..t.clone()
                    },
                    None => ChatThread { preview: String::new(), preview_kind: String::new(), preview_sender: String::new(), preview_sender_jid: String::new(), preview_status: String::new(), ..t.clone() },
                };
                set_thread(ctx, &t, next);
            }
            changed(ctx, user_id);
            Ok(json!({ "ok": true }))
        }
        "chatTarget" => {
            // The chat and its newest message, which WhatsApp wants when archiving.
            keys(p, &["threadId"])?;
            let t = thread(p.get("threadId"))?;
            let a = account(Some(&json!(t.account_id)))?;
            let latest = first_message(ctx, &t.id);
            Ok(json!({ "accountId": a.id, "platform": a.platform, "serverId": t.server_id, "kind": t.kind, "state": a.state, "chat": t.jid,
                "lastAt": latest.as_ref().map_or(0.0, |l| l.at), "lastId": latest.as_ref().map(|l| l.message_id.clone()).unwrap_or_default(),
                "lastSender": latest.as_ref().filter(|l| !l.from_me).map(|l| l.sender.clone()).unwrap_or_default(), "lastFromMe": latest.as_ref().is_some_and(|l| l.from_me) }))
        }
        "applyChatSetting" => {
            keys(p, &["threadId", "pinned", "archived", "mutedUntil"])?;
            let t = thread(p.get("threadId"))?;
            let mut next = t.clone();
            if let Some(b) = p.get("pinned").and_then(Value::as_bool) {
                next.pinned = b;
            }
            if let Some(b) = p.get("archived").and_then(Value::as_bool) {
                next.archived = b;
                if b {
                    next.pinned = false;
                }
            }
            if p.contains_key("mutedUntil") {
                next.muted_until = num(p.get("mutedUntil"))?;
            }
            set_thread(ctx, &t, next.clone());
            changed(ctx, user_id);
            let a = account(Some(&json!(t.account_id)))?;
            Ok(expose_thread(&next, Some(&ContactNames::new(ctx, user_id, &a.jid))))
        }
        "chatInfo" => {
            // The chat with its recent attachments (newest first) and how many messages Dar keeps.
            keys(p, &["threadId"])?;
            let t = thread(p.get("threadId"))?;
            let a = account(Some(&json!(t.account_id)))?;
            let mut media = Vec::new();
            let (mut messages, mut scanned) = (0, 0);
            let range = prefix(&t.id);
            for r in ctx.db.chat_messages().thread_key().filter(crate::bounds(&range)) {
                messages += 1;
                scanned += 1;
                if scanned > 20000 {
                    break;
                }
                if media.len() < 60
                    && let Some(m) = ctx.db.chat_media().id().find(r.id.clone())
                    && !r.deleted
                {
                    media.push(json!({ "messageId": r.message_id, "kind": m.kind, "mime": m.mime, "state": m.state, "fileName": m.file_name, "size": m.size, "at": m.at }));
                }
            }
            let server = if t.server_id.is_empty() { None } else { ctx.db.chat_servers().id().find(format!("{}/{}", a.id, t.server_id)) };
            Ok(json!({ "thread": expose_thread(&t, Some(&ContactNames::new(ctx, user_id, &a.jid))), "account": { "id": a.id, "name": a.name, "state": a.state, "platform": a.platform },
                "server": server.map(|s| json!({ "name": s.name })), "media": media, "messages": messages, "complete": scanned <= 20000 }))
        }
        "oldest" => {
            // The oldest message Dar has, from which the phone can send earlier ones.
            keys(p, &["threadId"])?;
            let t = thread(p.get("threadId"))?;
            let a = account(Some(&json!(t.account_id)))?;
            let range = prefix(&t.id);
            let oldest = ctx.db.chat_messages().thread_key().filter(crate::bounds(&range)).last();
            Ok(json!({ "accountId": a.id, "platform": a.platform, "state": a.state, "chat": t.jid, "message": oldest.map(|o| json!({ "id": o.message_id, "fromMe": o.from_me, "at": o.at })) }))
        }
        "newest" => {
            // The newest message Dar has (a pending one does not count), to catch up after it.
            keys(p, &["threadId"])?;
            let t = thread(p.get("threadId"))?;
            let a = account(Some(&json!(t.account_id)))?;
            let range = prefix(&t.id);
            let newest = ctx.db.chat_messages().thread_key().filter(crate::bounds(&range)).find(|r| r.status != "pending" && r.status != "failed");
            Ok(json!({ "accountId": a.id, "platform": a.platform, "state": a.state, "chat": t.jid, "message": newest.map(|o| json!({ "id": o.message_id, "at": o.at })) }))
        }
        "names" => {
            // Dar contact names for WhatsApp numbers (group members), keyed by JID.
            keys(p, &["accountId", "jids"])?;
            let a = account(p.get("accountId"))?;
            let Some(list) = p.get("jids").and_then(Value::as_array).filter(|l| l.len() <= 2000) else { return fail("invalidInput") };
            let names = ContactNames::new(ctx, user_id, &a.jid);
            let mut result = Map::new();
            for value in list {
                let id = text(Some(value), 120, true)?;
                if let Some(found) = names.find(&id) {
                    result.insert(id, found);
                }
            }
            Ok(Value::Object(result))
        }
        "contactSearch" => {
            // Dar contacts with a phone number, for starting a chat; numbers come back international.
            keys(p, &["accountId", "q"])?;
            let a = account(p.get("accountId"))?;
            let q = unicode_normalization::UnicodeNormalization::nfc(optional(p.get("q"), 100)?.as_str()).collect::<String>().to_lowercase().trim().to_owned();
            let own = a.jid.split('@').next().unwrap_or_default().to_owned();
            let q_digits: String = q.chars().filter(char::is_ascii_digit).collect();
            let mut found: Vec<Value> = Vec::new();
            for (book, r) in books_rows(ctx, user_id).into_iter().take(20000) {
                let Ok(data) = serde_json::from_str::<Value>(&r.projection) else { continue };
                let name = data["displayName"].as_str().map(|s| s.trim().to_owned()).unwrap_or_default();
                let Some(phones) = data["phones"].as_array().filter(|_| !name.is_empty()) else { continue };
                for phone in phones {
                    let digits = international_digits(&crate::text_of(Some(phone.get("value").unwrap_or(&json!("")))), &own);
                    if digits.is_empty() {
                        continue;
                    }
                    if !q.is_empty() && !name.to_lowercase().contains(&q) && !(q_digits.len() >= 3 && digits.contains(&q_digits)) {
                        continue;
                    }
                    found.push(json!({ "name": name, "phone": format!("+{digits}"), "label": phone["label"].as_str().unwrap_or_default(), "book": book, "href": r.href }));
                }
            }
            found.sort_by(|x, y| crate::locale_compare(x["name"].as_str().unwrap_or_default(), y["name"].as_str().unwrap_or_default()));
            found.truncate(30);
            Ok(json!({ "items": found }))
        }
        "deleteFailed" => {
            keys(p, &["threadId", "messageId"])?;
            let t = thread(p.get("threadId"))?;
            let r = ctx.db.chat_messages().id().find(format!("{}/{}", t.id, message_id(p.get("messageId"))?));
            let Some(r) = r.filter(|r| r.from_me && r.status == "failed") else { return fail("conflict") };
            ctx.db.chat_messages().id().delete(r.id);
            changed(ctx, user_id);
            Ok(json!({ "ok": true }))
        }
        _ => fail("invalidInput"),
    }
}

/// Bridge work: its key store, account states and incoming chats.
pub fn chat_worker(ctx: &ReducerContext, operation: &str, p: &Doc) -> Res<Value> {
    let account = |value: Option<&Value>| -> Res<ChatAccount> { ctx.db.chat_accounts().id().find(text(value, 64, true)?).ok_or_else(|| "notFound".to_owned()) };
    match operation {
        "accounts" => {
            keys(p, &[])?;
            Ok(json!({ "accounts": ctx.db.chat_accounts().iter().filter(|r| !r.jid.is_empty() && r.state != "loggedOut").map(|r| json!({ "id": r.id, "ownerId": r.owner_id, "state": r.state, "platform": r.platform })).collect::<Vec<_>>() }))
        }
        "kvLoad" => {
            keys(p, &["accountId", "cursor"])?;
            let a = account(p.get("accountId"))?;
            let cursor = optional(p.get("cursor"), 700)?;
            let base = format!("{}/c", a.id);
            if !cursor.is_empty() && !cursor.starts_with(&format!("{base}/")) {
                return fail("invalidInput");
            }
            let range = if cursor.is_empty() { prefix(&base) } else { after(&cursor, &base) };
            let mut entries: Vec<(String, String)> = Vec::new();
            let mut next = String::new();
            let mut size = 0usize;
            for r in ctx.db.chat_keys().scan_key().filter(crate::bounds(&range)) {
                if entries.len() >= 2000 || size > 4 * 1024 * 1024 {
                    next = entries.last().map(|e| format!("{base}/{}", e.0)).unwrap_or_default();
                    break;
                }
                size += r.value.encode_utf16().count();
                entries.push((r.scan_key[base.len() + 1..].to_owned(), r.value));
            }
            Ok(json!({ "entries": entries.iter().map(|(k, v)| json!({ "key": k, "value": v })).collect::<Vec<_>>(), "next": next }))
        }
        "kvGet" => {
            keys(p, &["accountId", "keys"])?;
            let a = account(p.get("accountId"))?;
            let Some(list) = p.get("keys").and_then(Value::as_array).filter(|l| l.len() <= 500) else { return fail("invalidInput") };
            let mut values = Map::new();
            for k in list {
                let k = text(Some(k), 600, true)?;
                if !valid_key(&k) {
                    return fail("invalidInput");
                }
                if let Some(r) = ctx.db.chat_keys().id().find(format!("{}/{k}", a.id)) {
                    values.insert(k, json!(r.value));
                }
            }
            Ok(json!({ "values": values }))
        }
        "kvCommit" => {
            // One transaction: whatsmeow relies on its decryption steps landing together.
            keys(p, &["accountId", "puts", "deletes"])?;
            let a = account(p.get("accountId"))?;
            let (Some(puts), Some(deletes)) = (p.get("puts").and_then(Value::as_array), p.get("deletes").and_then(Value::as_array)) else { return fail("invalidInput") };
            if puts.len() + deletes.len() > 20000 {
                return fail("invalidInput");
            }
            for k in deletes {
                let k = text(Some(k), 600, true)?;
                if !valid_key(&k) {
                    return fail("invalidInput");
                }
                ctx.db.chat_keys().id().delete(format!("{}/{k}", a.id));
            }
            for put in puts {
                let e = record(put)?;
                keys(e, &["key", "value", "lazy", "expiresAt"])?;
                let k = text(e.get("key"), 600, true)?;
                if !valid_key(&k) {
                    return fail("invalidInput");
                }
                let value = text(e.get("value"), 8 * 1024 * 1024, true)?;
                let expires_at = if e.contains_key("expiresAt") { num(e.get("expiresAt"))? } else { 0.0 };
                let id = format!("{}/{k}", a.id);
                let next = ChatKey {
                    id: id.clone(),
                    scan_key: format!("{}/{}/{k}", a.id, if e.get("lazy") == Some(&json!(true)) { "l" } else { "c" }),
                    expiry_key: if expires_at > 0.0 { format!("{:015}/{id}", expires_at.floor() as i64) } else { String::new() },
                    value,
                };
                if ctx.db.chat_keys().id().find(id).is_some() {
                    ctx.db.chat_keys().id().update(next);
                } else {
                    ctx.db.chat_keys().insert(next);
                }
            }
            Ok(json!({ "ok": true }))
        }
        "kvClear" => {
            keys(p, &["accountId"])?;
            clear_keys(ctx, &account(p.get("accountId"))?.id);
            Ok(json!({ "ok": true }))
        }
        "purge" => {
            // Expired event buffers and retry copies; bounded so one call stays small.
            keys(p, &[])?;
            let end = format!("{:015}", now(ctx) as i64);
            let range = (std::ops::Bound::Included("0".to_owned()), std::ops::Bound::Excluded(end));
            let mut removed = 0;
            for r in ctx.db.chat_keys().expiry_key().filter(crate::bounds(&range)).collect::<Vec<_>>() {
                if removed >= 5000 {
                    break;
                }
                ctx.db.chat_keys().id().delete(r.id);
                removed += 1;
            }
            Ok(json!({ "removed": removed }))
        }
        "state" => {
            keys(p, &["accountId", "state", "jid", "detail"])?;
            let a = account(p.get("accountId"))?;
            let state = p.get("state").and_then(Value::as_str).filter(|s| STATES.contains(s)).ok_or_else(|| "invalidInput".to_owned())?;
            let mut next = ChatAccount { state: state.into(), detail: optional(p.get("detail"), 200)?, ..a.clone() };
            if p.contains_key("jid") {
                next.jid = chat_id(&a, p.get("jid"))?;
                next.linked_at = now(ctx);
                // Linking a different number or Discord user to this account starts its chats afresh.
                if !a.jid.is_empty() && a.jid != next.jid {
                    delete_chats(ctx, &a.id);
                    next.messages = 0;
                    next.history_progress = 0;
                }
            }
            if state == "loggedOut" {
                next.importing = false;
            }
            ctx.db.chat_accounts().id().update(next);
            changed(ctx, &a.owner_id);
            Ok(json!({ "ok": true, "ownerId": a.owner_id }))
        }
        "mediaStored" => {
            keys(p, &["accountId", "id", "blobId", "size"])?;
            let a = account(p.get("accountId"))?;
            let Some(r) = ctx.db.chat_media().id().find(text(p.get("id"), 400, true)?).filter(|r| r.account_id == a.id) else { return fail("notFound") };
            let blob_id = text(p.get("blobId"), 64, true)?;
            let size = num(p.get("size"))?;
            if !is_blob_id(&blob_id) {
                return fail("invalidInput");
            }
            store_blob(ctx, &blob_id, size)?;
            if !r.blob_id.is_empty() && r.blob_id != blob_id {
                release_blob(ctx, &r.blob_id);
            }
            ctx.db.chat_media().id().update(ChatMedia { blob_id, size, state: "stored".into(), ..r });
            changed(ctx, &a.owner_id);
            Ok(json!({ "ok": true }))
        }
        "mediaState" => {
            keys(p, &["accountId", "id", "state"])?;
            let a = account(p.get("accountId"))?;
            let Some(r) = ctx.db.chat_media().id().find(text(p.get("id"), 400, true)?).filter(|r| r.account_id == a.id) else { return fail("notFound") };
            let state = crate::text_of(p.get("state"));
            if !["remote", "retrying", "expired", "failed"].contains(&state.as_str()) {
                return fail("invalidInput");
            }
            if r.state != "stored" {
                ctx.db.chat_media().id().update(ChatMedia { state, ..r });
                changed(ctx, &a.owner_id);
            }
            Ok(json!({ "ok": true }))
        }
        "mediaFind" => {
            // The phone sent expired media again: which attachment, so the runtime downloads it.
            keys(p, &["accountId", "chat", "messageId"])?;
            let a = account(p.get("accountId"))?;
            let chat = chat_id(&a, p.get("chat"))?;
            let r = ctx.db.chat_media().id().find(format!("{}/{chat}/{}", a.id, message_id(p.get("messageId"))?));
            Ok(match r {
                Some(r) => json!({ "id": r.id, "state": r.state, "chat": chat, "messageId": crate::text_of(p.get("messageId")) }),
                None => json!({ "id": "" }),
            })
        }
        "ingest" => {
            let a = account(p.get("accountId"))?;
            if a.jid.is_empty() {
                return fail("conflict");
            }
            let mut rest = p.clone();
            rest.remove("accountId");
            if a.platform == "discord" { discord_ingest(ctx, &a, &rest) } else { ingest(ctx, &a, &rest) }
        }
        _ => fail("invalidInput"),
    }
}

/// Removes a deleted Dar account's Chat data.
pub fn delete_chat_account(ctx: &ReducerContext, owner_id: &str) {
    for a in ctx.db.chat_accounts().owner_id().filter(owner_id).collect::<Vec<_>>() {
        delete_account_rows(ctx, &a);
    }
}

/// Chats, messages and downloaded attachments for backups. Keys are left out: a restored
/// account comes back unlinked, so two servers never use the same device.
pub fn chat_snapshot(ctx: &ReducerContext) -> Value {
    json!({
        "accounts": ctx.db.chat_accounts().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "threads": ctx.db.chat_threads().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "messages": ctx.db.chat_messages().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "media": ctx.db.chat_media().iter().map(|r| if r.state == "stored" { row(&r) } else { let state = if r.state == "retrying" { "remote".to_owned() } else { r.state.clone() }; row(&ChatMedia { blob_id: String::new(), state, ..r }) }).collect::<Vec<_>>(),
        "reactions": ctx.db.chat_reactions().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "servers": ctx.db.chat_servers().iter().map(|r| row(&r)).collect::<Vec<_>>(),
    })
}

pub fn chat_blob_ids(ctx: &ReducerContext) -> Vec<String> {
    let mut seen = HashSet::new();
    ctx.db.chat_media().iter().filter(|r| !r.blob_id.is_empty() && r.state == "stored").filter_map(|r| seen.insert(r.blob_id.clone()).then_some(r.blob_id)).collect()
}

fn with_defaults(value: &Value, defaults: &[(&str, Value)]) -> Value {
    let mut v = value.clone();
    if let Some(o) = v.as_object_mut() {
        for (k, d) in defaults {
            o.entry(k.to_string()).or_insert_with(|| d.clone());
        }
    }
    v
}

pub fn restore_chat(ctx: &ReducerContext, value: Option<&Value>) -> Res<()> {
    let Some(value) = value else { return Ok(()) };
    let list = |k: &str| value[k].as_array().cloned().unwrap_or_default();
    let parse = |v: Value| -> Res<_> { Ok(v) };
    for v in list("accounts") {
        let a: ChatAccount = serde_json::from_value(parse(v)?).map_err(|_| "invalidInput".to_owned())?;
        let state = if a.jid.is_empty() { "failed" } else { "loggedOut" };
        ctx.db.chat_accounts().insert(ChatAccount { state: state.into(), detail: "restored".into(), importing: false, ..a });
    }
    // Backups from before Discord lack its columns.
    for v in list("threads") {
        let defaults = [("serverId", json!("")), ("section", json!("")), ("order", json!("")), ("topic", json!("")), ("mentions", json!(0)), ("previewSenderJid", json!(""))];
        let t: ChatThread = serde_json::from_value(with_defaults(&v, &defaults)).map_err(|_| "invalidInput".to_owned())?;
        ctx.db.chat_threads().insert(t);
    }
    for v in list("messages") {
        ctx.db.chat_messages().insert(serde_json::from_value::<ChatMessage>(v).map_err(|_| "invalidInput".to_owned())?);
    }
    for v in list("media") {
        let m: ChatMedia = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        let m = if !m.blob_id.is_empty() && ctx.db.file_blobs().id().find(m.blob_id.clone()).is_none() { ChatMedia { blob_id: String::new(), state: "expired".into(), ..m } } else { m };
        ctx.db.chat_media().insert(m);
    }
    for v in list("reactions") {
        ctx.db.chat_reactions().insert(serde_json::from_value::<ChatReaction>(with_defaults(&v, &[("count", json!(1))])).map_err(|_| "invalidInput".to_owned())?);
    }
    for v in list("servers") {
        ctx.db.chat_servers().insert(serde_json::from_value::<ChatServer>(v).map_err(|_| "invalidInput".to_owned())?);
    }
    Ok(())
}

/// Chat in the shared search: chat names and message text, newest first, at most three
/// messages per chat. Returns false when it stopped at its own work limit.
pub fn chat_search(ctx: &ReducerContext, user_id: &str, s: &mut crate::global_search::Search) -> bool {
    let q = s.query().to_owned();
    let mut budget: i64 = 40000;
    for a in ctx.db.chat_accounts().owner_id().filter(user_id) {
        let names = ContactNames::new(ctx, user_id, &a.jid);
        let servers: HashMap<String, String> = ctx.db.chat_servers().account_id().filter(a.id.as_str()).map(|x| (x.server_id, x.name)).collect();
        let range = prefix(&a.id);
        for r in ctx.db.chat_threads().list_key().filter(crate::bounds(&range)) {
            let thread = expose_thread(&r, Some(&names));
            let href = format!("/chat?account={}&chat={}", crate::global_search::encode_uri_component(&a.id), crate::global_search::encode_uri_component(&r.id));
            let shown = thread["name"].as_str().unwrap_or_default().to_owned();
            let original = thread["whatsappName"].as_str().unwrap_or_default().to_owned();
            let title = if !r.server_id.is_empty() { format!("#{}", r.name) } else if !shown.is_empty() { shown.clone() } else { original.clone() };
            let kind = if !r.server_id.is_empty() { servers.get(&r.server_id).cloned().unwrap_or_else(|| "Server".into()) } else if r.kind == "group" { "Group".into() } else { "Chat".into() };
            if s.matches(&shown) || s.matches(&original) {
                s.found("Chat", r.id.clone(), title.clone(), format!("{kind} · {}", a.name), href.clone(), String::new());
            }
            let mut hits = 0;
            let range = prefix(&r.id);
            for m in ctx.db.chat_messages().thread_key().filter(crate::bounds(&range)) {
                budget -= 1;
                if budget < 0 {
                    return false;
                }
                if m.deleted || m.text.is_empty() || !s.matches(&m.text) {
                    continue;
                }
                let lower: String = unicode_normalization::UnicodeNormalization::nfc(m.text.as_str()).collect::<String>().to_lowercase();
                let units: Vec<u16> = lower.encode_utf16().collect();
                let needle: Vec<u16> = q.encode_utf16().collect();
                let at = if needle.is_empty() { 0 } else { units.windows(needle.len()).position(|w| w == needle.as_slice()).unwrap_or(0) };
                let source: Vec<u16> = m.text.encode_utf16().collect();
                let start = at.saturating_sub(40);
                let end = (at + needle.len() + 60).min(source.len());
                let piece = String::from_utf16_lossy(&source[start.min(source.len())..end]).split_whitespace().collect::<Vec<_>>().join(" ");
                let snippet = format!("{}{}{}", if start > 0 { "…" } else { "" }, piece, if end < source.len() { "…" } else { "" });
                let who = if m.from_me { "You".to_owned() } else if r.kind != "direct" { names.find(&m.sender).and_then(|c| c["name"].as_str().map(str::to_owned)).unwrap_or(m.sender_name.clone()) } else { String::new() };
                let detail = format!("{}{snippet}", if who.is_empty() { String::new() } else { format!("{who}: ") });
                s.found("Chat", m.id.clone(), title.clone(), detail, format!("{href}&message={}", crate::global_search::encode_uri_component(&m.message_id)), String::new());
                hits += 1;
                if hits >= 3 {
                    break;
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------------------
// Discord (`chat-discord.ts`).

fn valid_order(value: Option<&Value>) -> Res<String> {
    let order = optional(value, 80)?;
    if order.len() <= 80 && order.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'/') { Ok(order) } else { fail("invalidInput") }
}

fn count(value: Option<&Value>) -> Res<Option<u32>> {
    match value {
        None => Ok(None),
        v => Ok(Some(num(v)?.floor().clamp(0.0, 1_000_000.0) as u32)),
    }
}

struct DiscordChat {
    id: String,
    kind: String,
    name: String,
    server_id: String,
    section: String,
    order: String,
    topic: String,
    unread: Option<u32>,
    mentions: Option<u32>,
    muted_until: Option<f64>,
    at: f64,
}

fn discord_chat(value: &Value) -> Res<DiscordChat> {
    let c = record(value)?;
    keys(c, &["id", "kind", "name", "serverId", "section", "order", "topic", "unread", "mentions", "mutedUntil", "at"])?;
    let kind = text(c.get("kind"), 20, true)?;
    if !["direct", "group", "channel", "voice"].contains(&kind.as_str()) {
        return fail("invalidInput");
    }
    Ok(DiscordChat {
        id: discord_id(c.get("id"))?,
        kind,
        name: optional(c.get("name"), 200)?.trim().to_owned(),
        server_id: match c.get("serverId") {
            None => String::new(),
            Some(v) if v.as_str() == Some("") => String::new(),
            v => discord_id(v)?,
        },
        section: optional(c.get("section"), 200)?,
        order: valid_order(c.get("order"))?,
        topic: optional(c.get("topic"), 2048)?,
        unread: count(c.get("unread"))?,
        mentions: count(c.get("mentions"))?,
        muted_until: if c.contains_key("mutedUntil") { Some(num(c.get("mutedUntil"))?) } else { None },
        at: if c.contains_key("at") { num(c.get("at"))?.max(0.0) } else { 0.0 },
    })
}

fn discord_message(value: &Value) -> Res<MessageInput> {
    let m = record(value)?;
    keys(m, &["id", "chat", "sender", "senderName", "fromMe", "at", "kind", "text", "fileName", "target", "quotedId", "quotedSender", "quotedText", "status", "media", "nonce", "mention", "reactions"])?;
    let kind = text(m.get("kind"), 20, true)?;
    if !KINDS.contains(&kind.as_str()) && kind != "edit" && kind != "revoke" {
        return fail("invalidInput");
    }
    let Some(from_me) = m.get("fromMe").and_then(Value::as_bool) else { return fail("invalidInput") };
    let empty_or = |k: &str, f: &dyn Fn(Option<&Value>) -> Res<String>| -> Res<String> {
        match m.get(k) {
            None => Ok(String::new()),
            Some(v) if v.as_str() == Some("") => Ok(String::new()),
            v => f(v),
        }
    };
    let reactions = match m.get("reactions") {
        None => None,
        Some(v) => {
            let Some(list) = v.as_array().filter(|l| l.len() <= 100) else { return fail("invalidInput") };
            Some(
                list.iter()
                    .map(|item| {
                        let r = record(item)?;
                        keys(r, &["emoji", "count", "me"])?;
                        Ok((text(r.get("emoji"), 120, false)?, count(r.get("count"))?.unwrap_or(0), r.get("me") == Some(&json!(true))))
                    })
                    .collect::<Res<Vec<_>>>()?,
            )
        }
    };
    Ok(MessageInput {
        id: message_id(m.get("id"))?,
        chat: discord_id(m.get("chat"))?,
        sender: empty_or("sender", &discord_id)?,
        sender_name: optional(m.get("senderName"), 200)?,
        from_me,
        at: num(m.get("at"))?.max(0.0),
        kind,
        text: optional(m.get("text"), TEXT)?,
        file_name: optional(m.get("fileName"), 500)?,
        target: if m.contains_key("target") { discord_id(m.get("target"))? } else { String::new() },
        quoted_id: empty_or("quotedId", &discord_id)?,
        quoted_sender: optional(m.get("quotedSender"), 200)?,
        quoted_text: optional(m.get("quotedText"), 2000)?,
        status: String::new(),
        media: valid_media(m.get("media"))?,
        nonce: empty_or("nonce", &message_id)?,
        mention: m.get("mention") == Some(&json!(true)),
        reactions,
    })
}

/// Removes a chat with its messages, attachments and reactions; returns how many messages went.
fn delete_thread(ctx: &ReducerContext, t: &ChatThread) -> u32 {
    let mut removed = 0;
    let range = prefix(&t.id);
    for m in ctx.db.chat_messages().thread_key().filter(crate::bounds(&range)).collect::<Vec<_>>() {
        if let Some(media) = ctx.db.chat_media().id().find(m.id.clone()) {
            drop_media(ctx, &media);
        }
        for r in ctx.db.chat_reactions().message_row_id().filter(m.id.as_str()).collect::<Vec<_>>() {
            ctx.db.chat_reactions().id().delete(r.id);
        }
        ctx.db.chat_messages().id().delete(m.id);
        removed += 1;
    }
    ctx.db.chat_threads().id().delete(t.id.clone());
    removed
}

/// Discord reactions: the account's own per emoji, and a count for everyone else.
fn set_discord_reaction(ctx: &ReducerContext, account_id: &str, row_id: &str, emoji: &str, who: &str, count: f64, at: f64) {
    let id = format!("{row_id}|{who}|{emoji}");
    if count <= 0.0 {
        ctx.db.chat_reactions().id().delete(id);
        return;
    }
    let next = ChatReaction { id: id.clone(), message_row_id: row_id.into(), account_id: account_id.into(), sender: who.into(), sender_name: String::new(), from_me: who == "me", emoji: emoji.into(), at, count: count as u32 };
    if ctx.db.chat_reactions().id().find(id).is_some() {
        ctx.db.chat_reactions().id().update(next);
    } else {
        ctx.db.chat_reactions().insert(next);
    }
}

fn replace_reactions(ctx: &ReducerContext, account_id: &str, row_id: &str, list: &[(String, u32, bool)], at: f64) {
    for r in ctx.db.chat_reactions().message_row_id().filter(row_id).collect::<Vec<_>>() {
        ctx.db.chat_reactions().id().delete(r.id);
    }
    for (emoji, count, me) in list {
        if *me {
            set_discord_reaction(ctx, account_id, row_id, emoji, "me", 1.0, at);
        }
        set_discord_reaction(ctx, account_id, row_id, emoji, "*", *count as f64 - if *me { 1.0 } else { 0.0 }, at);
    }
}

/// Stores what the bridge sends for a Discord account: servers, channels (a complete list
/// removes channels the account no longer sees), direct messages, messages, reactions and reads.
fn discord_ingest(ctx: &ReducerContext, account: &ChatAccount, p: &Doc) -> Res<Value> {
    let kind = text(p.get("kind"), 20, true)?;
    let live = kind == "messages";
    let mut stored = account.messages as i64;
    let mut download: Vec<String> = Vec::new();
    let chats = p.get("chats").and_then(Value::as_array).map(|a| a.iter().map(discord_chat).collect::<Res<Vec<_>>>()).transpose()?.unwrap_or_default();
    if chats.len() > 5000 {
        return fail("operationTooLarge");
    }
    let upsert = |c: &DiscordChat, create: bool| {
        let id = format!("{}/{}", account.id, c.id);
        let old = ctx.db.chat_threads().id().find(id);
        if old.is_none() && !create {
            return;
        }
        let base = old.clone().unwrap_or_else(|| new_thread(account, &c.id, &c.kind, 0.0));
        let mut next = ChatThread { kind: c.kind.clone(), server_id: c.server_id.clone(), section: c.section.clone(), order: c.order.clone(), topic: c.topic.clone(), ..base };
        if !c.name.is_empty() {
            next.name = c.name.clone();
        }
        if let Some(u) = c.unread {
            next.unread = u;
        }
        if let Some(m) = c.mentions {
            next.mentions = m;
        }
        if let Some(m) = c.muted_until {
            next.muted_until = m;
        }
        if c.at > next.at {
            next.at = c.at;
            next.list_key = list_key(&account.id, c.at, &c.id);
        }
        match old {
            Some(old) => set_thread(ctx, &old, next),
            None => {
                ctx.db.chat_threads().insert(next);
            }
        }
    };
    match kind.as_str() {
        "servers" => {
            let Some(list) = p.get("servers").and_then(Value::as_array) else { return fail("invalidInput") };
            if list.len() > 1000 {
                return fail("operationTooLarge");
            }
            let mut ids = Vec::new();
            for v in list {
                let s = record(v)?;
                keys(s, &["id", "name", "order", "mutedUntil"])?;
                let server_id = discord_id(s.get("id"))?;
                let name = optional(s.get("name"), 200)?.trim().to_owned();
                let next = ChatServer {
                    id: format!("{}/{server_id}", account.id),
                    account_id: account.id.clone(),
                    server_id: server_id.clone(),
                    name: if name.is_empty() { "Server".into() } else { name },
                    order: valid_order(s.get("order"))?,
                    muted_until: if s.contains_key("mutedUntil") { num(s.get("mutedUntil"))? } else { 0.0 },
                };
                if ctx.db.chat_servers().id().find(next.id.clone()).is_some() {
                    ctx.db.chat_servers().id().update(next);
                } else {
                    ctx.db.chat_servers().insert(next);
                }
                ids.push(server_id);
            }
            if p.get("complete") == Some(&json!(true)) {
                // Servers the account left go with their channels.
                for r in ctx.db.chat_servers().account_id().filter(account.id.as_str()).collect::<Vec<_>>() {
                    if !ids.contains(&r.server_id) {
                        ctx.db.chat_servers().id().delete(r.id);
                    }
                }
                for t in ctx.db.chat_threads().account_id().filter(account.id.as_str()).collect::<Vec<_>>() {
                    if !t.server_id.is_empty() && !ids.contains(&t.server_id) {
                        stored -= delete_thread(ctx, &t) as i64;
                    }
                }
            }
        }
        "channels" => {
            let server_id = discord_id(p.get("serverId"))?;
            for c in &chats {
                if c.server_id != server_id || c.kind != "channel" && c.kind != "voice" {
                    return fail("invalidInput");
                }
                upsert(c, true);
            }
            if p.get("complete") == Some(&json!(true)) {
                let keep: HashSet<&String> = chats.iter().map(|c| &c.id).collect();
                for t in ctx.db.chat_threads().account_id().filter(account.id.as_str()).collect::<Vec<_>>() {
                    if t.server_id == server_id && !keep.contains(&t.jid) {
                        stored -= delete_thread(ctx, &t) as i64;
                    }
                }
            }
        }
        "chats" => {
            for c in &chats {
                if !c.server_id.is_empty() || c.kind != "direct" && c.kind != "group" {
                    return fail("invalidInput");
                }
                upsert(c, true);
            }
        }
        "messages" | "history" => {
            // A new direct message brings its chat along; server channels must already be known.
            for c in &chats {
                if !c.server_id.is_empty() || c.kind != "direct" && c.kind != "group" {
                    return fail("invalidInput");
                }
                upsert(c, true);
            }
            let mut messages = p.get("messages").and_then(Value::as_array).map(|a| a.iter().map(discord_message).collect::<Res<Vec<_>>>()).transpose()?.unwrap_or_default();
            if messages.len() > 2000 {
                return fail("operationTooLarge");
            }
            messages.sort_by(|a, b| a.at.total_cmp(&b.at));
            for m in &messages {
                let Some(t) = ctx.db.chat_threads().id().find(format!("{}/{}", account.id, m.chat)) else { continue };
                if m.kind == "edit" || m.kind == "revoke" {
                    if let Some(target) = ctx.db.chat_messages().id().find(format!("{}/{}", t.id, m.target))
                        && !target.deleted
                        && (m.kind == "revoke" || target.text != m.text)
                    {
                        revise_message(ctx, &t, &target, (m.kind == "edit").then(|| m.text.clone()), m.kind == "revoke");
                    }
                    continue;
                }
                let id = format!("{}/{}", t.id, m.id);
                // Our own message coming back under its real ID replaces the pending copy stored under the nonce.
                if m.from_me && !m.nonce.is_empty() && m.nonce != m.id && let Some(pending) = ctx.db.chat_messages().id().find(format!("{}/{}", t.id, m.nonce)) {
                    ctx.db.chat_messages().id().delete(pending.id);
                    stored -= 1;
                }
                if let Some(old) = ctx.db.chat_messages().id().find(id.clone()) {
                    // Fetched again: reaction counts may have changed meanwhile.
                    if let Some(list) = &m.reactions
                        && !old.deleted
                    {
                        replace_reactions(ctx, &account.id, &old.id, list, m.at);
                    }
                    continue;
                }
                if stored >= MESSAGES_PER_ACCOUNT as i64 {
                    break;
                }
                insert_message(ctx, account, &t, m, if m.from_me { "sent".into() } else { String::new() });
                stored += 1;
                if let Some(list) = m.reactions.as_ref().filter(|l| !l.is_empty()) {
                    replace_reactions(ctx, &account.id, &id, list, m.at);
                }
                if let Some(media) = &m.media {
                    insert_media(ctx, account, &id, m, media);
                    if live && ["image", "sticker", "voice"].contains(&m.kind.as_str()) && media.size <= AUTO_DOWNLOAD {
                        download.push(id.clone());
                    }
                }
                let current = ctx.db.chat_threads().id().find(t.id.clone()).expect("thread");
                let mut next = current.clone();
                if m.at >= current.at {
                    next.at = m.at;
                    next.list_key = list_key(&account.id, m.at, &current.jid);
                    next.preview = if m.text.is_empty() { m.file_name.clone() } else { m.text.clone() };
                    next.preview_kind = m.kind.clone();
                    next.preview_from_me = m.from_me;
                    next.preview_sender = if current.kind != "direct" && !m.from_me { m.sender_name.clone() } else { String::new() };
                    next.preview_sender_jid = String::new();
                    next.preview_status = if m.from_me { "sent".into() } else { String::new() };
                }
                if live {
                    // Sending from another device means the chat was read there.
                    if m.from_me {
                        next.unread = 0;
                        next.mentions = 0;
                    } else {
                        next.unread = (current.unread + 1).min(1_000_000);
                        if m.mention {
                            next.mentions = (current.mentions + 1).min(1_000_000);
                        }
                    }
                }
                set_thread(ctx, &current, next);
            }
        }
        "reactions" => {
            let Some(items) = p.get("items").and_then(Value::as_array) else { return fail("invalidInput") };
            if items.len() > 1000 {
                return fail("operationTooLarge");
            }
            for item in items {
                let r = record(item)?;
                keys(r, &["chat", "id", "emoji", "me", "delta", "clear"])?;
                let row_id = format!("{}/{}/{}", account.id, discord_id(r.get("chat"))?, discord_id(r.get("id"))?);
                let emoji = optional(r.get("emoji"), 120)?;
                if ctx.db.chat_messages().id().find(row_id.clone()).is_none_or(|m| m.deleted) {
                    continue;
                }
                if r.get("clear") == Some(&json!(true)) {
                    for old in ctx.db.chat_reactions().message_row_id().filter(row_id.as_str()).collect::<Vec<_>>() {
                        if emoji.is_empty() || old.emoji == emoji {
                            ctx.db.chat_reactions().id().delete(old.id);
                        }
                    }
                    continue;
                }
                if emoji.is_empty() {
                    return fail("invalidInput");
                }
                let delta = num(r.get("delta"))?.signum();
                let delta = if num(r.get("delta"))? == 0.0 { 0.0 } else { delta };
                if r.get("me") == Some(&json!(true)) {
                    set_discord_reaction(ctx, &account.id, &row_id, &emoji, "me", if delta > 0.0 { 1.0 } else { 0.0 }, now(ctx));
                } else {
                    let others = ctx.db.chat_reactions().id().find(format!("{row_id}|*|{emoji}")).map_or(0.0, |o| o.count as f64);
                    set_discord_reaction(ctx, &account.id, &row_id, &emoji, "*", others + delta, now(ctx));
                }
            }
        }
        "read" => {
            if let Some(t) = ctx.db.chat_threads().id().find(format!("{}/{}", account.id, discord_id(p.get("chat"))?))
                && (t.unread > 0 || t.mentions > 0)
            {
                set_thread(ctx, &t, ChatThread { unread: 0, mentions: 0, ..t.clone() });
            }
        }
        _ => return fail("invalidInput"),
    }
    let current = ctx.db.chat_accounts().id().find(account.id.clone()).unwrap_or_else(|| account.clone());
    ctx.db.chat_accounts().id().update(ChatAccount { messages: stored.max(0) as u32, synced_at: now(ctx), ..current });
    changed(ctx, &account.owner_id);
    Ok(json!({ "ok": true, "download": download }))
}

/// The servers of a Discord account with their channels, for the sidebar.
fn discord_servers(ctx: &ReducerContext, account: &ChatAccount) -> Vec<Value> {
    let mut channels: HashMap<String, Vec<ChatThread>> = HashMap::new();
    for t in ctx.db.chat_threads().account_id().filter(account.id.as_str()) {
        if !t.server_id.is_empty() {
            channels.entry(t.server_id.clone()).or_default().push(t);
        }
    }
    let at = now(ctx);
    let quiet = |until: f64| until == -1.0 || until > at;
    let mut servers: Vec<ChatServer> = ctx.db.chat_servers().account_id().filter(account.id.as_str()).collect();
    servers.sort_by(|a, b| crate::locale_compare(&a.order, &b.order).then_with(|| crate::locale_compare(&a.name, &b.name)));
    servers
        .iter()
        .map(|s| {
            let mut list = channels.remove(&s.server_id).unwrap_or_default();
            list.sort_by(|a, b| crate::locale_compare(&a.order, &b.order));
            let unread = !quiet(s.muted_until) && list.iter().any(|t| t.unread > 0 && !quiet(t.muted_until));
            json!({ "id": s.id, "serverId": s.server_id, "name": s.name, "mutedUntil": s.muted_until, "unread": unread,
                "mentions": list.iter().map(|t| t.mentions).sum::<u32>(), "channels": list.iter().map(|t| expose_thread(t, None)).collect::<Vec<_>>() })
        })
        .collect()
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_chat(ctx: &mut ProcedureContext, sessionId: String, operation: String, document: String) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let user = crate::from_session(tx, &sessionId)?;
        if tx.db.app_passwords().session_id().find(sessionId.clone()).is_some() {
            return fail("forbidden");
        }
        if document.len() > 256 * 1024 {
            return fail("operationTooLarge");
        }
        chat_operation(tx, &user.id, &operation, &crate::document(&document)?)
    })
}

/// The chat bridge's key store and incoming chats, through the runtime.
#[spacetimedb::procedure]
pub fn service_chat_worker(ctx: &mut ProcedureContext, operation: String, document: String) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        if document.len() > 24 * 1024 * 1024 {
            return fail("operationTooLarge");
        }
        chat_worker(tx, &operation, &crate::document(&document)?)
    })
}

#[spacetimedb::view(accessor = my_chat_changes, public)]
pub fn my_chat_changes(ctx: &spacetimedb::ViewContext) -> Option<ChatChange> {
    let user = crate::user_for_view(ctx)?;
    Some(ctx.db.chat_changes().user_id().find(user.id.clone()).unwrap_or(ChatChange { user_id: user.id, revision: 0 }))
}
