//! Mail: administrator-configured providers, accounts, mailboxes and messages. Every message is
//! mirrored as its original `.eml` file in the owner's Files (`docs/mail/<account>/<mailbox>/`);
//! these rows index those files for lists and search. The runtime reads message content from the
//! files, talks to the mail servers and reports here; the module also tells it what changed in
//! the mirror (moved, deleted or added `.eml` files) so the server can follow.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::app_settings::app_enabled;
use crate::files::{Unlock, event, find_key, fits_quota, key, meter, move_for, trash_for};
use crate::tables::*;
use crate::{Res, fail, name_of, now, unused_uuid};

type Doc = Map<String, Value>;

pub const DEFAULT_MAIL_FOLDER: &str = "docs/mail";
const ACCOUNTS: usize = 20;
const MAILBOXES: usize = 1000;
const PAGE: usize = 50;
const SYNC_MS: f64 = 5.0 * 60_000.0;
const LEASE_MS: f64 = 10.0 * 60_000.0;
const MESSAGE_BYTES: f64 = 64.0 * 1024.0 * 1024.0;
pub const READ_OPERATIONS: [&str; 5] = ["overview", "messages", "message", "providers", "locate"];
const ROLES: [&str; 8] = [
    "", "inbox", "sent", "drafts", "trash", "junk", "archive", "folder",
];

fn keys(p: &Doc, allowed: &[&str]) -> Res<()> {
    if p.keys().any(|k| !allowed.contains(&k.as_str())) {
        fail("invalidInput")
    } else {
        Ok(())
    }
}

fn text(value: Option<&Value>, max: usize, allow_empty: bool) -> Res<String> {
    let Some(s) = value.and_then(Value::as_str) else {
        return fail("invalidInput");
    };
    if !allow_empty && s.trim().is_empty() || s.contains('\0') || s.encode_utf16().count() > max {
        return fail("invalidInput");
    }
    Ok(s.to_owned())
}

fn optional_text(value: Option<&Value>, max: usize) -> Res<String> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        v => text(v, max, true),
    }
}

fn flag(value: Option<&Value>) -> Res<bool> {
    match value {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        _ => fail("invalidInput"),
    }
}

fn number(value: Option<&Value>, min: f64, max: f64) -> Res<f64> {
    match value.and_then(Value::as_f64) {
        Some(n) if n.is_finite() && n >= min && n <= max => Ok(n),
        _ => fail("invalidInput"),
    }
}

fn id_list(value: Option<&Value>, max: usize) -> Res<Vec<String>> {
    let Some(list) = value.and_then(Value::as_array) else {
        return fail("invalidInput");
    };
    if list.is_empty() || list.len() > max {
        return fail("invalidInput");
    }
    list.iter().map(|v| text(Some(v), 100, false)).collect()
}

/// Newest first in ascending index order.
fn newest(date: f64) -> String {
    format!(
        "{:015}",
        (999_999_999_999_999.0 - date.clamp(0.0, 999_999_999_999_999.0)) as u64
    )
}

fn counted_unread(role: &str) -> bool {
    !matches!(role, "sent" | "drafts" | "trash" | "junk")
}

/// A message with its list keys derived from its state and its mailbox's role.
fn keyed(m: MailMessage, role: &str) -> MailMessage {
    let order = newest(m.date);
    let owner_key = format!("{}/{order}/{}", m.owner_id, m.id);
    MailMessage {
        mailbox_key: format!("{}/{order}/{}", m.mailbox_id, m.id),
        role_key: if role.is_empty() || role == "folder" {
            String::new()
        } else {
            format!("{}/{role}/{order}/{}", m.owner_id, m.id)
        },
        unread_key: if !m.seen && counted_unread(role) {
            owner_key.clone()
        } else {
            String::new()
        },
        flagged_key: if m.flagged && role != "trash" {
            owner_key.clone()
        } else {
            String::new()
        },
        remote_key: format!(
            "{}/{}/{}",
            m.account_id,
            m.mailbox_id,
            if m.remote.is_empty() {
                format!("pending:{}", m.id)
            } else {
                m.remote.clone()
            }
        ),
        owner_key,
        ..m
    }
}

fn mailbox_role(ctx: &ReducerContext, mailbox_id: &str) -> String {
    ctx.db
        .mail_mailboxes()
        .id()
        .find(mailbox_id.to_owned())
        .map(|m| m.role)
        .unwrap_or_default()
}

fn recount(ctx: &ReducerContext, mailbox_id: &str) {
    let Some(mailbox) = ctx.db.mail_mailboxes().id().find(mailbox_id.to_owned()) else {
        return;
    };
    let (mut total, mut unread) = (0u32, 0u32);
    for m in ctx.db.mail_messages().mailbox_id().filter(mailbox_id) {
        total += 1;
        if !m.seen {
            unread += 1;
        }
    }
    if mailbox.total != total || mailbox.unread != unread {
        ctx.db.mail_mailboxes().id().update(MailMailbox {
            total,
            unread,
            ..mailbox
        });
    }
}

// ---------- Files mirror ----------

fn preference(ctx: &ReducerContext, user_id: &str, name: &str) -> Option<String> {
    let value: Value = serde_json::from_str(
        &ctx.db
            .pim_preferences()
            .user_id()
            .find(user_id.to_owned())?
            .value,
    )
    .ok()?;
    value.get(name)?.as_str().map(str::to_owned)
}

pub fn mail_folder_parts(ctx: &ReducerContext, user_id: &str) -> Vec<String> {
    preference(ctx, user_id, "mailFolder")
        .and_then(|v| crate::folders::photo_folder_parts(&v))
        .unwrap_or_else(|| DEFAULT_MAIL_FOLDER.split('/').map(str::to_owned).collect())
}

/// A live (not trashed) folder entry the user owns.
fn live_folder(ctx: &ReducerContext, owner_id: &str, id: &str) -> Option<FileEntry> {
    if id.is_empty() {
        return None;
    }
    ctx.db
        .file_entries()
        .id()
        .find(id.to_owned())
        .filter(|f| f.owner_id == owner_id && f.kind == "folder" && !f.trashed)
}

/// A child folder by name, created when missing; a taken name gets " (2)" and so on.
fn child_folder(ctx: &ReducerContext, owner_id: &str, parent_id: &str, name: &str) -> Res<String> {
    let name = name_of(name).unwrap_or_else(|_| "Mailbox".into());
    if let Some(existing) = find_key(ctx, &key(owner_id, parent_id, &name)) {
        if existing.kind == "folder" && !existing.trashed {
            return Ok(existing.id);
        }
    }
    let free = crate::folders::free_name(ctx, owner_id, parent_id, &name)?;
    let id = unused_uuid(ctx);
    crate::files::create_folder_for(ctx, owner_id, &id, parent_id, &free, Unlock::Nobody)?;
    Ok(id)
}

/// A new folder with this name (never an existing one), for a new account.
fn fresh_folder(ctx: &ReducerContext, owner_id: &str, parent_id: &str, name: &str) -> Res<String> {
    let name = name_of(name).unwrap_or_else(|_| "Mail".into());
    let free = crate::folders::free_name(ctx, owner_id, parent_id, &name)?;
    let id = unused_uuid(ctx);
    crate::files::create_folder_for(ctx, owner_id, &id, parent_id, &free, Unlock::Nobody)?;
    Ok(id)
}

fn mirror_root(ctx: &ReducerContext, owner_id: &str) -> Res<String> {
    Ok(
        crate::folders::own_folder(ctx, owner_id, &mail_folder_parts(ctx, owner_id), true)?
            .unwrap_or_default(),
    )
}

/// The account's folder, recreated when it was deleted or trashed.
fn account_folder(ctx: &ReducerContext, account: &MailAccount) -> Res<String> {
    if live_folder(ctx, &account.owner_id, &account.folder_id).is_some() {
        return Ok(account.folder_id.clone());
    }
    let root = mirror_root(ctx, &account.owner_id)?;
    let id = fresh_folder(ctx, &account.owner_id, &root, &account.name)?;
    ctx.db.mail_accounts().id().update(MailAccount {
        folder_id: id.clone(),
        ..account.clone()
    });
    Ok(id)
}

/// The mailbox's folder, recreated (with its parents) when missing.
fn mailbox_folder(
    ctx: &ReducerContext,
    account: &MailAccount,
    mailbox: &MailMailbox,
) -> Res<String> {
    if live_folder(ctx, &account.owner_id, &mailbox.folder_id).is_some() {
        return Ok(mailbox.folder_id.clone());
    }
    let parent = match ctx.db.mail_mailboxes().id().find(mailbox.parent_id.clone()) {
        Some(p) if p.id != mailbox.id => mailbox_folder(ctx, account, &p)?,
        _ => account_folder(
            ctx,
            &ctx.db
                .mail_accounts()
                .id()
                .find(account.id.clone())
                .unwrap_or_else(|| account.clone()),
        )?,
    };
    let id = child_folder(ctx, &account.owner_id, &parent, &mailbox.name)?;
    ctx.db.mail_mailboxes().id().update(MailMailbox {
        folder_id: id.clone(),
        ..mailbox.clone()
    });
    Ok(id)
}

/// `YYYY-MM-DD HHMM Subject.eml`, safe as a file name.
fn file_name(date: f64, subject: &str) -> String {
    let iso = crate::iso_string(date.max(0.0));
    let stamp = format!("{} {}{}", &iso[0..10], &iso[11..13], &iso[14..16]);
    let mut clean: String = subject
        .chars()
        .map(|c| {
            if c.is_control() || c == '/' || c == '\\' {
                ' '
            } else {
                c
            }
        })
        .collect();
    clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean: String = clean
        .chars()
        .take(120)
        .collect::<String>()
        .trim_end_matches(['.', ' '])
        .to_owned();
    format!(
        "{stamp} {}.eml",
        if clean.is_empty() {
            "No subject"
        } else {
            &clean
        }
    )
}

/// A new `.eml` file in Files from content the runtime stored under its digest.
fn put_file(
    ctx: &ReducerContext,
    owner_id: &str,
    parent_id: &str,
    name: &str,
    digest: &str,
    size: f64,
    modified_at: f64,
) -> Res<FileEntry> {
    let usage = meter(ctx, owner_id);
    if !fits_quota(&usage, size) {
        return fail("quotaExceeded");
    }
    crate::blobs::store_blob(ctx, digest, size)?;
    let name = crate::folders::free_name(ctx, owner_id, parent_id, &name_of(name)?)?;
    let at = now(ctx);
    let id = unused_uuid(ctx);
    let entry = FileEntry {
        id: id.clone(),
        owner_id: owner_id.into(),
        parent_id: parent_id.into(),
        name: name.clone(),
        kind: "file".into(),
        blob_id: digest.into(),
        size,
        media_type: "message/rfc822".into(),
        revision: 1,
        modified_at: if modified_at > 0.0 && modified_at < at + 86_400_000.0 {
            modified_at
        } else {
            at
        },
        trashed: false,
        favorite: false,
        list_key: key(owner_id, parent_id, &name),
    };
    ctx.db.file_entries().insert(entry.clone());
    ctx.db.versions().insert(Version {
        id: unused_uuid(ctx),
        file_id: id.clone(),
        blob_id: digest.into(),
        size,
        media_type: "message/rfc822".into(),
        created_at: at,
    });
    ctx.db.usage().user_id().update(Usage {
        used: usage.used + size,
        ..usage
    });
    event(ctx, owner_id, &id, "uploaded", None);
    Ok(entry)
}

fn trash_file(ctx: &ReducerContext, owner_id: &str, file_id: &str) {
    if let Some(f) = ctx
        .db
        .file_entries()
        .id()
        .find(file_id.to_owned())
        .filter(|f| !f.trashed && f.owner_id == owner_id)
    {
        let _ = trash_for(ctx, owner_id, &f.id, f.revision, Unlock::Nobody);
    }
}

/// Where an entry sits relative to an account folder.
enum Place {
    /// Inside the account folder; the folder holding it.
    Inside(String),
    /// It or a folder above it is in deleted files.
    Trashed,
    /// Somewhere else in Files.
    Outside,
}

fn place(ctx: &ReducerContext, entry: &FileEntry, root: &str) -> Place {
    if entry.trashed {
        return Place::Trashed;
    }
    let mut current = entry.parent_id.clone();
    for _ in 0..64 {
        if current.is_empty() {
            return Place::Outside;
        }
        if current == root {
            return Place::Inside(entry.parent_id.clone());
        }
        match ctx.db.file_entries().id().find(current.clone()) {
            Some(f) if f.trashed => return Place::Trashed,
            Some(f) => current = f.parent_id,
            None => return Place::Outside,
        }
    }
    Place::Outside
}

// ---------- Rows as JSON ----------

fn provider_json(p: &MailProvider, full: bool) -> Value {
    let doc: Value = serde_json::from_str(&p.document).unwrap_or(json!({}));
    let mut value = json!({
        "id": p.id, "name": p.name, "source": p.source, "enabled": p.enabled,
        "protocol": doc["protocol"], "auth": doc["auth"], "domains": doc.get("domains").cloned().unwrap_or(json!([])),
    });
    if full {
        value["document"] = doc;
        value["hasSecret"] = json!(!p.secret.is_empty());
        value["accounts"] = json!(0);
    }
    value
}

fn mailbox_json(m: &MailMailbox) -> Value {
    json!({ "id": m.id, "name": m.name, "role": m.role, "parentId": m.parent_id, "total": m.total, "unread": m.unread, "folderId": m.folder_id })
}

fn account_json(ctx: &ReducerContext, a: &MailAccount) -> Value {
    let provider = ctx.db.mail_providers().id().find(a.provider_id.clone());
    let doc: Value = provider
        .as_ref()
        .and_then(|p| serde_json::from_str(&p.document).ok())
        .unwrap_or(json!({}));
    let mut mailboxes: Vec<MailMailbox> =
        ctx.db.mail_mailboxes().account_id().filter(&a.id).collect();
    mailboxes.sort_by(|x, y| {
        role_rank(&x.role)
            .cmp(&role_rank(&y.role))
            .then_with(|| crate::locale_compare(&x.name, &y.name))
    });
    json!({
        "id": a.id, "providerId": a.provider_id, "providerName": provider.as_ref().map(|p| p.name.clone()).unwrap_or_default(),
        "protocol": doc["protocol"], "auth": doc["auth"], "providerAvailable": provider.as_ref().is_some_and(|p| p.enabled),
        "name": a.name, "address": a.address, "displayName": a.display_name, "username": a.username, "color": a.color, "ordering": a.ordering,
        "folderId": a.folder_id, "status": a.status, "lastError": a.last_error, "errorCount": a.error_count, "syncedAt": a.synced_at, "signature": a.signature,
        "mailboxes": mailboxes.iter().map(mailbox_json).collect::<Vec<_>>(),
    })
}

fn role_rank(role: &str) -> u8 {
    match role {
        "inbox" => 0,
        "drafts" => 1,
        "sent" => 2,
        "archive" => 3,
        "junk" => 4,
        "trash" => 5,
        _ => 6,
    }
}

fn summary(ctx: &ReducerContext, m: &MailMessage) -> Value {
    json!({
        "id": m.id, "accountId": m.account_id, "mailboxId": m.mailbox_id, "subject": m.subject, "fromName": m.from_name, "fromAddress": m.from_address,
        "recipients": serde_json::from_str::<Value>(&m.recipients).unwrap_or(json!({ "to": [], "cc": [] })), "date": m.date, "snippet": m.snippet,
        "seen": m.seen, "flagged": m.flagged, "answered": m.answered, "draft": m.draft, "attachments": m.attachments, "size": m.size,
        "messageId": m.message_id, "fileId": m.file_id,
        "threadCount": if m.thread_key.is_empty() { 1 } else { ctx.db.mail_messages().thread_key().filter(&m.thread_key).count() },
    })
}

/// The account with the secrets the runtime needs to reach its server.
fn work_json(ctx: &ReducerContext, a: &MailAccount) -> Value {
    let provider = ctx.db.mail_providers().id().find(a.provider_id.clone());
    json!({
        "id": a.id, "ownerId": a.owner_id, "name": a.name, "address": a.address, "displayName": a.display_name, "username": a.username,
        "credential": a.credential, "status": a.status,
        "provider": provider.map(|p| json!({ "id": p.id, "name": p.name, "source": p.source, "enabled": p.enabled, "secret": p.secret,
            "document": serde_json::from_str::<Value>(&p.document).unwrap_or(json!({})) })),
        "mailboxes": ctx.db.mail_mailboxes().account_id().filter(&a.id).map(|m| json!({
            "id": m.id, "remote": m.remote, "name": m.name, "role": m.role, "parentId": m.parent_id, "validity": m.validity, "folderId": m.folder_id,
        })).collect::<Vec<_>>(),
    })
}

fn own_account(ctx: &ReducerContext, user_id: &str, id: Option<&Value>) -> Res<MailAccount> {
    let id = text(id, 100, false)?;
    ctx.db
        .mail_accounts()
        .id()
        .find(id)
        .filter(|a| a.owner_id == user_id)
        .ok_or_else(|| "notFound".to_owned())
}

fn own_message(ctx: &ReducerContext, user_id: &str, id: &str) -> Res<MailMessage> {
    ctx.db
        .mail_messages()
        .id()
        .find(id.to_owned())
        .filter(|m| m.owner_id == user_id)
        .ok_or_else(|| "notFound".to_owned())
}

fn provider_document(value: Option<&Value>) -> Res<Value> {
    let Some(doc) = value.filter(|v| v.is_object()) else {
        return fail("invalidInput");
    };
    if serde_json::to_string(doc).map_or(0, |s| s.len()) > 16 * 1024 {
        return fail("invalidInput");
    }
    let protocol = doc["protocol"].as_str().unwrap_or_default();
    let auth = doc["auth"].as_str().unwrap_or_default();
    if !matches!(protocol, "imap" | "jmap") || !matches!(auth, "password" | "token" | "oauth") {
        return fail("invalidInput");
    }
    let server = |v: &Value| -> bool {
        v["host"].as_str().is_some_and(|h| {
            !h.is_empty()
                && h.len() <= 253
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
        }) && v["port"]
            .as_f64()
            .is_some_and(|p| (1.0..=65535.0).contains(&p) && p.fract() == 0.0)
            && matches!(v["security"].as_str(), Some("tls" | "starttls"))
    };
    let https = |v: &Value| {
        v.as_str().is_some_and(|u| {
            (u.starts_with("https://") || u.starts_with("http://")) && u.len() <= 2048
        })
    };
    let ok = match protocol {
        "imap" => server(&doc["imap"]) && server(&doc["smtp"]) && auth != "token",
        _ => https(&doc["jmap"]["url"]),
    } && (auth != "oauth"
        || https(&doc["oauth"]["authorizeUrl"])
            && https(&doc["oauth"]["tokenUrl"])
            && doc["oauth"]["clientId"]
                .as_str()
                .is_some_and(|c| !c.is_empty() && c.len() <= 500));
    if !ok {
        return fail("invalidInput");
    }
    Ok(doc.clone())
}

fn valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && id.as_bytes()[0].is_ascii_alphanumeric()
}

// ---------- Browser operations ----------

pub fn mail_operation(
    ctx: &ReducerContext,
    user: &Account,
    operation: &str,
    p: &Doc,
) -> Res<Value> {
    if !app_enabled(ctx, "mail") {
        return fail("notFound");
    }
    let user_id = user.id.as_str();
    let at = now(ctx);
    match operation {
        "overview" => {
            keys(p, &[])?;
            let mut providers: Vec<MailProvider> = ctx
                .db
                .mail_providers()
                .iter()
                .filter(|p| p.enabled)
                .collect();
            providers.sort_by(|a, b| {
                a.ordering
                    .cmp(&b.ordering)
                    .then_with(|| crate::locale_compare(&a.name, &b.name))
            });
            let mut accounts: Vec<MailAccount> =
                ctx.db.mail_accounts().owner_id().filter(user_id).collect();
            accounts.sort_by(|a, b| {
                a.ordering
                    .cmp(&b.ordering)
                    .then(a.created_at.total_cmp(&b.created_at))
            });
            let count = |index: &str| -> usize {
                let range = crate::prefix(&format!("{user_id}/{index}"));
                ctx.db
                    .mail_messages()
                    .role_key()
                    .filter(crate::bounds(&range))
                    .count()
            };
            let unread_range = crate::prefix(&format!("{user_id}/"));
            Ok(json!({
                "providers": providers.iter().map(|p| provider_json(p, false)).collect::<Vec<_>>(),
                "accounts": accounts.iter().map(|a| account_json(ctx, a)).collect::<Vec<_>>(),
                "counts": {
                    "inbox": ctx.db.mail_messages().role_key().filter(crate::bounds(&crate::prefix(&format!("{user_id}/inbox/")))).filter(|m| !m.seen).count(),
                    "unread": ctx.db.mail_messages().unread_key().filter(crate::bounds(&unread_range)).count(),
                    "flagged": ctx.db.mail_messages().flagged_key().filter(crate::bounds(&unread_range)).count(),
                    "drafts": count("drafts/"),
                },
                "mailFolder": mail_folder_parts(ctx, user_id).join("/"),
                "administrator": user.administrator,
            }))
        }
        "messages" => {
            keys(p, &["view", "id", "cursor", "limit"])?;
            let view = text(p.get("view"), 20, false)?;
            let cursor = optional_text(p.get("cursor"), 300)?;
            let limit = match p.get("limit") {
                None => PAGE,
                v => number(v, 1.0, 200.0)? as usize,
            };
            let (index, prefix): (&str, String) = match view.as_str() {
                "inbox" | "drafts" | "sent" | "archive" | "trash" | "junk" => {
                    ("role", format!("{user_id}/{view}/"))
                }
                "unread" => ("unread", format!("{user_id}/")),
                "flagged" => ("flagged", format!("{user_id}/")),
                "all" => ("owner", format!("{user_id}/")),
                "mailbox" => {
                    let id = text(p.get("id"), 100, false)?;
                    let mailbox = ctx
                        .db
                        .mail_mailboxes()
                        .id()
                        .find(id)
                        .filter(|m| m.owner_id == user_id)
                        .ok_or("notFound")?;
                    ("mailbox", format!("{}/", mailbox.id))
                }
                "account" => {
                    let account = own_account(ctx, user_id, p.get("id"))?;
                    let inbox = ctx
                        .db
                        .mail_mailboxes()
                        .account_id()
                        .filter(&account.id)
                        .find(|m| m.role == "inbox")
                        .ok_or("notFound")?;
                    ("mailbox", format!("{}/", inbox.id))
                }
                _ => return fail("invalidInput"),
            };
            if !cursor.is_empty() && !cursor.starts_with(&prefix) {
                return fail("invalidInput");
            }
            let range = crate::window_of(&prefix, (!cursor.is_empty()).then_some(cursor.as_str()));
            let bounds = crate::bounds(&range);
            let rows: Vec<MailMessage> = match index {
                "role" => ctx
                    .db
                    .mail_messages()
                    .role_key()
                    .filter(bounds)
                    .take(limit + 1)
                    .collect(),
                "unread" => ctx
                    .db
                    .mail_messages()
                    .unread_key()
                    .filter(bounds)
                    .take(limit + 1)
                    .collect(),
                "flagged" => ctx
                    .db
                    .mail_messages()
                    .flagged_key()
                    .filter(bounds)
                    .take(limit + 1)
                    .collect(),
                "owner" => ctx
                    .db
                    .mail_messages()
                    .owner_key()
                    .filter(bounds)
                    .take(limit + 1)
                    .collect(),
                _ => ctx
                    .db
                    .mail_messages()
                    .mailbox_key()
                    .filter(bounds)
                    .take(limit + 1)
                    .collect(),
            };
            let next = (rows.len() > limit).then(|| {
                let last = &rows[limit - 1];
                match index {
                    "role" => last.role_key.clone(),
                    "unread" => last.unread_key.clone(),
                    "flagged" => last.flagged_key.clone(),
                    "owner" => last.owner_key.clone(),
                    _ => last.mailbox_key.clone(),
                }
            });
            Ok(
                json!({ "items": rows.iter().take(limit).map(|m| summary(ctx, m)).collect::<Vec<_>>(), "cursor": next }),
            )
        }
        "message" => {
            keys(p, &["id"])?;
            let m = own_message(ctx, user_id, &text(p.get("id"), 100, false)?)?;
            let file = ctx
                .db
                .file_entries()
                .id()
                .find(m.file_id.clone())
                .filter(|f| f.owner_id == user_id);
            let mut thread: Vec<MailMessage> = if m.thread_key.is_empty() {
                vec![m.clone()]
            } else {
                ctx.db
                    .mail_messages()
                    .thread_key()
                    .filter(&m.thread_key)
                    .collect()
            };
            thread.sort_by(|a, b| a.date.total_cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
            let blob = |r: &MailMessage| -> String {
                ctx.db
                    .file_entries()
                    .id()
                    .find(r.file_id.clone())
                    .filter(|f| f.owner_id == user_id && !f.blob_id.is_empty())
                    .map_or(r.blob_id.clone(), |f| f.blob_id)
            };
            let account = ctx
                .db
                .mail_accounts()
                .id()
                .find(m.account_id.clone())
                .ok_or("notFound")?;
            let mailbox = ctx.db.mail_mailboxes().id().find(m.mailbox_id.clone());
            let mut value = summary(ctx, &m);
            value["blobId"] = json!(blob(&m));
            value["fileName"] = json!(file.as_ref().map(|f| f.name.clone()).unwrap_or_default());
            value["mailboxName"] =
                json!(mailbox.as_ref().map(|b| b.name.clone()).unwrap_or_default());
            value["mailboxRole"] =
                json!(mailbox.as_ref().map(|b| b.role.clone()).unwrap_or_default());
            value["accountName"] = json!(account.name);
            value["thread"] = json!(
                thread
                    .iter()
                    .map(|r| {
                        let mut v = summary(ctx, r);
                        v["blobId"] = json!(blob(r));
                        v["mailboxRole"] = json!(mailbox_role(ctx, &r.mailbox_id));
                        v
                    })
                    .collect::<Vec<_>>()
            );
            Ok(value)
        }
        "locate" => {
            keys(p, &["id"])?;
            let m = own_message(ctx, user_id, &text(p.get("id"), 100, false)?)?;
            let file = ctx
                .db
                .file_entries()
                .id()
                .find(m.file_id)
                .filter(|f| f.owner_id == user_id && !f.trashed)
                .ok_or("notFound")?;
            Ok(json!({ "fileId": file.id, "parentId": file.parent_id, "name": file.name }))
        }
        "prepare" => {
            // What the runtime needs for an action on the server: the accounts with their
            // secrets and the messages' remote identities, checked against this user.
            keys(p, &["ids", "accountId"])?;
            let mut groups: Vec<(MailAccount, Vec<MailMessage>)> = Vec::new();
            if p.contains_key("ids") {
                for id in id_list(p.get("ids"), 500)? {
                    let m = own_message(ctx, user_id, &id)?;
                    match groups.iter_mut().find(|(a, _)| a.id == m.account_id) {
                        Some((_, list)) => list.push(m),
                        None => groups.push((
                            ctx.db
                                .mail_accounts()
                                .id()
                                .find(m.account_id.clone())
                                .ok_or("notFound")?,
                            vec![m],
                        )),
                    }
                }
            } else {
                groups.push((own_account(ctx, user_id, p.get("accountId"))?, Vec::new()));
            }
            Ok(json!({ "groups": groups.iter().map(|(a, list)| {
                let mut v = work_json(ctx, a);
                v["messages"] = json!(list.iter().map(|m| json!({ "id": m.id, "remote": m.remote, "mailboxId": m.mailbox_id, "seen": m.seen, "flagged": m.flagged, "blobId": m.blob_id })).collect::<Vec<_>>());
                v
            }).collect::<Vec<_>>() }))
        }
        "createAccount" => {
            keys(
                p,
                &[
                    "providerId",
                    "name",
                    "address",
                    "displayName",
                    "username",
                    "credential",
                    "color",
                ],
            )?;
            let provider_id = text(p.get("providerId"), 64, false)?;
            if !ctx
                .db
                .mail_providers()
                .id()
                .find(provider_id.clone())
                .is_some_and(|p| p.enabled)
            {
                return fail("notFound");
            }
            let existing: Vec<MailAccount> =
                ctx.db.mail_accounts().owner_id().filter(user_id).collect();
            if existing.len() >= ACCOUNTS {
                return fail("quotaExceeded");
            }
            let address = text(p.get("address"), 320, false)?.trim().to_owned();
            if !address.contains('@') || address.contains(char::is_whitespace) {
                return fail("invalidInput");
            }
            if existing
                .iter()
                .any(|a| a.provider_id == provider_id && a.address.eq_ignore_ascii_case(&address))
            {
                return fail("alreadyExists");
            }
            let name = name_of(&text(p.get("name"), 100, false)?)?;
            let root = mirror_root(ctx, user_id)?;
            let folder_id = fresh_folder(ctx, user_id, &root, &name)?;
            let account = MailAccount {
                id: unused_uuid(ctx),
                owner_id: user_id.into(),
                provider_id,
                name,
                address,
                display_name: optional_text(p.get("displayName"), 200)?.trim().to_owned(),
                username: optional_text(p.get("username"), 320)?,
                credential: text(p.get("credential"), 64 * 1024, false)?,
                color: match p.get("color") {
                    None => (existing.len() % 6) as u32,
                    v => number(v, 0.0, 5.0)? as u32,
                },
                ordering: existing.iter().map(|a| a.ordering + 1).max().unwrap_or(0),
                folder_id,
                status: "syncing".into(),
                last_error: String::new(),
                error_count: 0,
                synced_at: 0.0,
                next_sync: at,
                due_key: due_key(at),
                signature: String::new(),
                created_at: at,
            };
            ctx.db.mail_accounts().insert(account.clone());
            Ok(account_json(ctx, &account))
        }
        "updateAccount" => {
            keys(
                p,
                &[
                    "id",
                    "name",
                    "displayName",
                    "signature",
                    "color",
                    "credential",
                    "ordering",
                ],
            )?;
            let mut a = own_account(ctx, user_id, p.get("id"))?;
            if p.contains_key("name") {
                a.name = name_of(&text(p.get("name"), 100, false)?)?;
            }
            if p.contains_key("displayName") {
                a.display_name = optional_text(p.get("displayName"), 200)?.trim().to_owned();
            }
            if p.contains_key("signature") {
                a.signature = optional_text(p.get("signature"), 4000)?;
            }
            if p.contains_key("color") {
                a.color = number(p.get("color"), 0.0, 5.0)? as u32;
            }
            if p.contains_key("ordering") {
                a.ordering = number(p.get("ordering"), 0.0, 1000.0)? as u32;
            }
            if p.contains_key("credential") {
                a.credential = text(p.get("credential"), 64 * 1024, false)?;
                a.status = "syncing".into();
                a.last_error = String::new();
                a.error_count = 0;
                a.next_sync = at;
                a.due_key = due_key(at);
            }
            ctx.db.mail_accounts().id().update(a.clone());
            Ok(account_json(ctx, &a))
        }
        "removeAccount" => {
            keys(p, &["id", "keepFiles"])?;
            let a = own_account(ctx, user_id, p.get("id"))?;
            let keep = match p.get("keepFiles") {
                None => true,
                v => flag(v)?,
            };
            remove_account(ctx, &a, keep);
            Ok(json!({ "removed": true }))
        }
        "syncNow" => {
            keys(p, &["id"])?;
            let list: Vec<MailAccount> = match p.get("id") {
                None | Some(Value::Null) => {
                    ctx.db.mail_accounts().owner_id().filter(user_id).collect()
                }
                v => vec![own_account(ctx, user_id, v)?],
            };
            for a in list {
                ctx.db.mail_accounts().id().update(MailAccount {
                    next_sync: at,
                    due_key: due_key(at),
                    ..a
                });
            }
            Ok(json!({ "ok": true }))
        }
        "providers" | "saveProvider" | "deleteProvider" => {
            if !user.administrator {
                return fail("forbidden");
            }
            provider_admin(ctx, operation, p)
        }
        _ => fail("invalidInput"),
    }
}

fn provider_admin(ctx: &ReducerContext, operation: &str, p: &Doc) -> Res<Value> {
    let at = now(ctx);
    match operation {
        "providers" => {
            keys(p, &[])?;
            let mut list: Vec<MailProvider> = ctx.db.mail_providers().iter().collect();
            list.sort_by(|a, b| {
                a.ordering
                    .cmp(&b.ordering)
                    .then_with(|| crate::locale_compare(&a.name, &b.name))
            });
            Ok(json!({ "items": list.iter().map(|r| {
                let mut v = provider_json(r, true);
                v["accounts"] = json!(ctx.db.mail_accounts().provider_id().filter(&r.id).count());
                v
            }).collect::<Vec<_>>() }))
        }
        "saveProvider" => {
            keys(
                p,
                &["id", "name", "document", "secret", "enabled", "create"],
            )?;
            let id = text(p.get("id"), 64, false)?;
            if !valid_provider_id(&id) {
                return fail("invalidInput");
            }
            let existing = ctx.db.mail_providers().id().find(id.clone());
            if existing.as_ref().is_some_and(|e| e.source != "admin") {
                return fail("forbidden");
            }
            if flag(p.get("create"))? && existing.is_some() {
                return fail("alreadyExists");
            }
            let document = provider_document(p.get("document"))?;
            let secret = match p.get("secret") {
                None | Some(Value::Null) => existing
                    .as_ref()
                    .map(|e| e.secret.clone())
                    .unwrap_or_default(),
                v => optional_text(v, 8192)?,
            };
            let row = MailProvider {
                id: id.clone(),
                name: text(p.get("name"), 100, false)?.trim().to_owned(),
                source: "admin".into(),
                document: document.to_string(),
                secret,
                enabled: match p.get("enabled") {
                    None => true,
                    v => flag(v)?,
                },
                ordering: existing.as_ref().map_or_else(
                    || {
                        ctx.db
                            .mail_providers()
                            .iter()
                            .map(|r| r.ordering + 1)
                            .max()
                            .unwrap_or(0)
                    },
                    |e| e.ordering,
                ),
                modified_at: at,
            };
            if existing.is_some() {
                ctx.db.mail_providers().id().update(row.clone());
            } else {
                ctx.db.mail_providers().insert(row.clone());
            }
            Ok(provider_json(&row, true))
        }
        _ => {
            keys(p, &["id"])?;
            let id = text(p.get("id"), 64, false)?;
            let existing = ctx
                .db
                .mail_providers()
                .id()
                .find(id.clone())
                .ok_or("notFound")?;
            if existing.source != "admin" {
                return fail("forbidden");
            }
            if ctx
                .db
                .mail_accounts()
                .provider_id()
                .filter(&id)
                .next()
                .is_some()
            {
                return fail("conflict");
            }
            ctx.db.mail_providers().id().delete(id);
            Ok(json!({ "deleted": true }))
        }
    }
}

fn due_key(at: f64) -> String {
    format!("{:016}", at.max(0.0).floor() as u64)
}

fn remove_account(ctx: &ReducerContext, a: &MailAccount, keep_files: bool) {
    for m in ctx
        .db
        .mail_messages()
        .account_id()
        .filter(&a.id)
        .collect::<Vec<_>>()
    {
        ctx.db.mail_messages().id().delete(m.id);
    }
    for b in ctx
        .db
        .mail_mailboxes()
        .account_id()
        .filter(&a.id)
        .collect::<Vec<_>>()
    {
        ctx.db.mail_mailboxes().id().delete(b.id);
    }
    if !keep_files {
        trash_file(ctx, &a.owner_id, &a.folder_id);
    }
    ctx.db.mail_accounts().id().delete(a.id.clone());
}

pub fn delete_mail_account(ctx: &ReducerContext, owner_id: &str) {
    for a in ctx
        .db
        .mail_accounts()
        .owner_id()
        .filter(owner_id)
        .collect::<Vec<_>>()
    {
        remove_account(ctx, &a, true);
    }
}

pub fn mail_search(ctx: &ReducerContext, user_id: &str, s: &mut crate::global_search::Search) {
    let range = crate::prefix(&format!("{user_id}/"));
    for m in ctx
        .db
        .mail_messages()
        .owner_key()
        .filter(crate::bounds(&range))
    {
        if !s.visit() {
            return;
        }
        let from = if m.from_name.is_empty() {
            m.from_address.clone()
        } else {
            m.from_name.clone()
        };
        if s.matches(&m.subject) || s.matches(&from) {
            let title = if m.subject.is_empty() {
                "No subject".to_owned()
            } else {
                m.subject.clone()
            };
            s.found(
                "Mail",
                format!("mail-{}", m.id),
                title,
                from,
                format!("/mail?message={}", m.id),
                String::new(),
            );
        }
    }
}

// ---------- Worker steps ----------

fn message_meta(
    v: &Value,
) -> Res<(
    String,
    String,
    String,
    String,
    String,
    String,
    f64,
    String,
    bool,
)> {
    let o = v.as_object().ok_or("invalidInput")?;
    let recipients = o
        .get("recipients")
        .cloned()
        .unwrap_or(json!({ "to": [], "cc": [] }));
    let recipients = serde_json::to_string(&recipients).map_err(|_| "invalidInput".to_owned())?;
    if recipients.len() > 32 * 1024 {
        return fail("invalidInput");
    }
    let clip = |value: Option<&Value>, max: usize| -> String {
        let s = value
            .and_then(Value::as_str)
            .unwrap_or_default()
            .replace('\0', "");
        s.chars().take(max).collect()
    };
    Ok((
        clip(o.get("messageId"), 998),
        clip(o.get("threadId"), 998),
        clip(o.get("subject"), 998),
        clip(o.get("fromName"), 300),
        clip(o.get("fromAddress"), 320),
        recipients,
        number(o.get("date"), 0.0, 1e15).unwrap_or(0.0),
        clip(o.get("snippet"), 300),
        flag(o.get("attachments")).unwrap_or(false),
    ))
}

pub fn mail_worker(ctx: &ReducerContext, operation: &str, p: &Doc) -> Res<Value> {
    let at = now(ctx);
    let account_of = |p: &Doc| -> Res<MailAccount> {
        ctx.db
            .mail_accounts()
            .id()
            .find(text(p.get("accountId"), 100, false)?)
            .ok_or_else(|| "notFound".to_owned())
    };
    match operation {
        "publishProviders" => {
            keys(p, &["providers"])?;
            let list = p
                .get("providers")
                .and_then(Value::as_array)
                .ok_or("invalidInput")?;
            let mut seen = HashSet::new();
            for (n, v) in list.iter().enumerate() {
                let id = text(v.get("id"), 64, false)?;
                if !valid_provider_id(&id) || !seen.insert(id.clone()) {
                    return fail("invalidInput");
                }
                let document = provider_document(v.get("document"))?;
                let name = text(v.get("name"), 100, false)?.trim().to_owned();
                let row = MailProvider {
                    id: id.clone(),
                    name,
                    source: "nix".into(),
                    document: document.to_string(),
                    secret: String::new(),
                    enabled: true,
                    ordering: n as u32,
                    modified_at: at,
                };
                match ctx.db.mail_providers().id().find(id) {
                    Some(old) if old.source == "admin" => return fail("conflict"),
                    Some(_) => {
                        ctx.db.mail_providers().id().update(row);
                    }
                    None => {
                        ctx.db.mail_providers().insert(row);
                    }
                }
            }
            for old in ctx
                .db
                .mail_providers()
                .iter()
                .filter(|r| r.source == "nix" && !seen.contains(&r.id))
                .collect::<Vec<_>>()
            {
                // Accounts keep working only while their provider exists; a removed Nix provider
                // is kept disabled so its accounts say why they stopped.
                ctx.db.mail_providers().id().update(MailProvider {
                    enabled: false,
                    ..old
                });
            }
            Ok(json!({ "ok": true }))
        }
        "due" => {
            keys(p, &["limit"])?;
            if !app_enabled(ctx, "mail") {
                return Ok(json!({ "accounts": [] }));
            }
            let limit = number(p.get("limit"), 1.0, 20.0)? as usize;
            let range = (
                std::ops::Bound::Included(due_key(0.0)),
                std::ops::Bound::Included(due_key(at)),
            );
            let mut due = Vec::new();
            for a in ctx
                .db
                .mail_accounts()
                .due_key()
                .filter(crate::bounds(&range))
                .collect::<Vec<_>>()
            {
                if ctx
                    .db
                    .accounts()
                    .id()
                    .find(a.owner_id.clone())
                    .is_none_or(|u| !u.enabled)
                {
                    continue;
                }
                let next = at + LEASE_MS;
                let leased = MailAccount {
                    next_sync: next,
                    due_key: due_key(next),
                    ..a
                };
                ctx.db.mail_accounts().id().update(leased.clone());
                due.push(work_json(ctx, &leased));
                if due.len() >= limit {
                    break;
                }
            }
            Ok(json!({ "accounts": due }))
        }
        "account" => {
            keys(p, &["accountId"])?;
            Ok(work_json(ctx, &account_of(p)?))
        }
        "provider" => {
            keys(p, &["id"])?;
            let r = ctx
                .db
                .mail_providers()
                .id()
                .find(text(p.get("id"), 64, false)?)
                .ok_or("notFound")?;
            Ok(
                json!({ "id": r.id, "name": r.name, "source": r.source, "enabled": r.enabled, "secret": r.secret, "document": serde_json::from_str::<Value>(&r.document).unwrap_or(json!({})) }),
            )
        }
        "credential" => {
            keys(p, &["accountId", "credential"])?;
            let a = account_of(p)?;
            ctx.db.mail_accounts().id().update(MailAccount {
                credential: text(p.get("credential"), 64 * 1024, false)?,
                ..a
            });
            Ok(json!({ "ok": true }))
        }
        "plan" => {
            keys(p, &["accountId"])?;
            plan(ctx, &account_of(p)?)
        }
        "mailboxes" => {
            keys(p, &["accountId", "mailboxes", "bound"])?;
            let a = account_of(p)?;
            store_mailboxes(ctx, &a, p.get("mailboxes"), p.get("bound"))
        }
        "known" => {
            keys(p, &["accountId", "mailboxId"])?;
            let a = account_of(p)?;
            let mailbox_id = text(p.get("mailboxId"), 100, false)?;
            let list: Vec<Value> = ctx
                .db
                .mail_messages()
                .mailbox_id()
                .filter(&mailbox_id)
                .filter(|m| m.account_id == a.id)
                .map(|m| {
                    json!([
                        m.id,
                        m.remote,
                        m.seen,
                        m.flagged,
                        m.answered,
                        m.draft,
                        m.message_id
                    ])
                })
                .collect();
            Ok(json!({ "messages": list }))
        }
        "validity" => {
            keys(p, &["accountId", "mailboxId", "validity"])?;
            let a = account_of(p)?;
            let mailbox = ctx
                .db
                .mail_mailboxes()
                .id()
                .find(text(p.get("mailboxId"), 100, false)?)
                .filter(|m| m.account_id == a.id)
                .ok_or("notFound")?;
            // Every UID was reassigned: the next pass mirrors the mailbox again.
            for m in ctx
                .db
                .mail_messages()
                .mailbox_id()
                .filter(&mailbox.id)
                .collect::<Vec<_>>()
            {
                trash_file(ctx, &a.owner_id, &m.file_id);
                ctx.db.mail_messages().id().delete(m.id);
            }
            ctx.db.mail_mailboxes().id().update(MailMailbox {
                validity: text(p.get("validity"), 100, true)?,
                total: 0,
                unread: 0,
                ..mailbox
            });
            Ok(json!({ "ok": true }))
        }
        "store" => {
            keys(p, &["accountId", "mailboxId", "messages"])?;
            let a = account_of(p)?;
            let mailbox = ctx
                .db
                .mail_mailboxes()
                .id()
                .find(text(p.get("mailboxId"), 100, false)?)
                .filter(|m| m.account_id == a.id)
                .ok_or("notFound")?;
            let list = p
                .get("messages")
                .and_then(Value::as_array)
                .ok_or("invalidInput")?;
            if list.len() > 200 {
                return fail("invalidInput");
            }
            let folder = mailbox_folder(ctx, &a, &mailbox)?;
            let mut stored = 0;
            let mut ids = Vec::new();
            for v in list {
                let remote = text(v.get("remote"), 300, false)?;
                if ctx
                    .db
                    .mail_messages()
                    .remote_key()
                    .find(format!("{}/{}/{remote}", a.id, mailbox.id))
                    .is_some()
                {
                    continue;
                }
                let (
                    message_id,
                    thread,
                    subject,
                    from_name,
                    from_address,
                    recipients,
                    date,
                    snippet,
                    attachments,
                ) = message_meta(v)?;
                let size = number(v.get("size"), 0.0, MESSAGE_BYTES)?.floor();
                let blob = text(v.get("blobId"), 100, false)?;
                // A message added in Files (or sent from Mail) that this pass now sees on the server.
                let pending = (!message_id.is_empty())
                    .then(|| {
                        ctx.db
                            .mail_messages()
                            .mailbox_id()
                            .filter(&mailbox.id)
                            .find(|m| m.remote.is_empty() && m.message_id == message_id)
                    })
                    .flatten();
                let file_id = match (&pending, v.get("fileId")) {
                    (Some(m), _) => m.file_id.clone(),
                    (None, Some(f)) if !f.is_null() => {
                        let f = ctx
                            .db
                            .file_entries()
                            .id()
                            .find(text(Some(f), 100, false)?)
                            .filter(|f| f.owner_id == a.owner_id)
                            .ok_or("notFound")?;
                        f.id
                    }
                    _ => {
                        put_file(
                            ctx,
                            &a.owner_id,
                            &folder,
                            &file_name(date, &subject),
                            &blob,
                            size,
                            date,
                        )?
                        .id
                    }
                };
                let row = MailMessage {
                    id: pending
                        .as_ref()
                        .map_or_else(|| unused_uuid(ctx), |m| m.id.clone()),
                    owner_id: a.owner_id.clone(),
                    account_id: a.id.clone(),
                    mailbox_id: mailbox.id.clone(),
                    remote,
                    remote_key: String::new(),
                    file_id,
                    blob_id: blob,
                    message_id: message_id.clone(),
                    thread_key: format!(
                        "{}/{}",
                        a.owner_id,
                        if thread.is_empty() {
                            if message_id.is_empty() {
                                unused_uuid(ctx)
                            } else {
                                message_id.clone()
                            }
                        } else {
                            thread
                        }
                    ),
                    subject,
                    from_name,
                    from_address,
                    recipients,
                    date,
                    snippet,
                    seen: flag(v.get("seen"))?,
                    flagged: flag(v.get("flagged"))?,
                    answered: flag(v.get("answered"))?,
                    draft: flag(v.get("draft"))?,
                    attachments,
                    size,
                    owner_key: String::new(),
                    mailbox_key: String::new(),
                    role_key: String::new(),
                    unread_key: String::new(),
                    flagged_key: String::new(),
                };
                let row = keyed(row, &mailbox.role);
                ids.push(row.id.clone());
                if pending.is_some() {
                    ctx.db.mail_messages().id().update(row);
                } else {
                    ctx.db.mail_messages().insert(row);
                }
                stored += 1;
            }
            recount(ctx, &mailbox.id);
            Ok(json!({ "stored": stored, "ids": ids }))
        }
        "flags" => {
            keys(p, &["accountId", "changes"])?;
            let a = account_of(p)?;
            let list = p
                .get("changes")
                .and_then(Value::as_array)
                .ok_or("invalidInput")?;
            let mut touched = HashSet::new();
            for v in list {
                let Some(m) = ctx
                    .db
                    .mail_messages()
                    .id()
                    .find(text(v.get("id"), 100, false)?)
                    .filter(|m| m.account_id == a.id)
                else {
                    continue;
                };
                let pick = |k: &str, old: bool| -> Res<bool> {
                    if v.get(k).is_some() {
                        flag(v.get(k))
                    } else {
                        Ok(old)
                    }
                };
                let next = MailMessage {
                    seen: pick("seen", m.seen)?,
                    flagged: pick("flagged", m.flagged)?,
                    answered: pick("answered", m.answered)?,
                    draft: pick("draft", m.draft)?,
                    ..m.clone()
                };
                touched.insert(next.mailbox_id.clone());
                let role = mailbox_role(ctx, &next.mailbox_id);
                ctx.db.mail_messages().id().update(keyed(next, &role));
            }
            for id in touched {
                recount(ctx, &id);
            }
            Ok(json!({ "ok": true }))
        }
        "remove" => {
            keys(p, &["accountId", "ids", "trashFiles"])?;
            let a = account_of(p)?;
            let trash = flag(p.get("trashFiles"))?;
            let mut touched = HashSet::new();
            for id in p
                .get("ids")
                .and_then(Value::as_array)
                .ok_or("invalidInput")?
            {
                let Some(m) = ctx
                    .db
                    .mail_messages()
                    .id()
                    .find(text(Some(id), 100, false)?)
                    .filter(|m| m.account_id == a.id)
                else {
                    continue;
                };
                if trash {
                    trash_file(ctx, &a.owner_id, &m.file_id);
                }
                touched.insert(m.mailbox_id.clone());
                ctx.db.mail_messages().id().delete(m.id);
            }
            for id in touched {
                recount(ctx, &id);
            }
            Ok(json!({ "ok": true }))
        }
        "moved" => {
            // The server moved messages; with `moveFiles` (an action in Mail) their files follow.
            keys(p, &["accountId", "moves", "moveFiles"])?;
            let a = account_of(p)?;
            let move_files = flag(p.get("moveFiles"))?;
            let mut touched = HashSet::new();
            for v in p
                .get("moves")
                .and_then(Value::as_array)
                .ok_or("invalidInput")?
            {
                let Some(m) = ctx
                    .db
                    .mail_messages()
                    .id()
                    .find(text(v.get("id"), 100, false)?)
                    .filter(|m| m.account_id == a.id)
                else {
                    continue;
                };
                let target = ctx
                    .db
                    .mail_mailboxes()
                    .id()
                    .find(text(v.get("mailboxId"), 100, false)?)
                    .filter(|b| b.account_id == a.id)
                    .ok_or("notFound")?;
                let remote = optional_text(v.get("remote"), 300)?;
                if move_files {
                    let folder = mailbox_folder(ctx, &a, &target)?;
                    if let Some(f) = ctx
                        .db
                        .file_entries()
                        .id()
                        .find(m.file_id.clone())
                        .filter(|f| !f.trashed)
                    {
                        let name = crate::folders::free_name(ctx, &a.owner_id, &folder, &f.name)?;
                        move_for(
                            ctx,
                            &a.owner_id,
                            &f.id,
                            &folder,
                            f.revision,
                            Some(&name),
                            Unlock::Nobody,
                        )?;
                    }
                }
                touched.insert(m.mailbox_id.clone());
                touched.insert(target.id.clone());
                let next = MailMessage {
                    mailbox_id: target.id.clone(),
                    remote,
                    ..m
                };
                if let Some(blob) = v.get("blobId").filter(|b| !b.is_null()) {
                    let blob = text(Some(blob), 100, false)?;
                    let row = MailMessage {
                        blob_id: blob,
                        ..next
                    };
                    ctx.db.mail_messages().id().update(keyed(row, &target.role));
                } else {
                    ctx.db
                        .mail_messages()
                        .id()
                        .update(keyed(next, &target.role));
                }
            }
            for id in touched {
                recount(ctx, &id);
            }
            Ok(json!({ "ok": true }))
        }
        "pending" => {
            // A message sent or saved from Mail, or added in Files, before the server lists it:
            // indexed now so it shows at once; the next pass gives it its server identity.
            keys(p, &["accountId", "mailboxId", "message"])?;
            let a = account_of(p)?;
            let mailbox = ctx
                .db
                .mail_mailboxes()
                .id()
                .find(text(p.get("mailboxId"), 100, false)?)
                .filter(|m| m.account_id == a.id)
                .ok_or("notFound")?;
            let v = p.get("message").ok_or("invalidInput")?;
            let (
                message_id,
                thread,
                subject,
                from_name,
                from_address,
                recipients,
                date,
                snippet,
                attachments,
            ) = message_meta(v)?;
            let blob = text(v.get("blobId"), 100, false)?;
            let size = number(v.get("size"), 0.0, MESSAGE_BYTES)?.floor();
            let file_id = match v.get("fileId") {
                Some(f) if !f.is_null() => {
                    ctx.db
                        .file_entries()
                        .id()
                        .find(text(Some(f), 100, false)?)
                        .filter(|f| f.owner_id == a.owner_id)
                        .ok_or("notFound")?
                        .id
                }
                _ => {
                    let folder = mailbox_folder(ctx, &a, &mailbox)?;
                    put_file(
                        ctx,
                        &a.owner_id,
                        &folder,
                        &file_name(date, &subject),
                        &blob,
                        size,
                        date,
                    )?
                    .id
                }
            };
            let id = unused_uuid(ctx);
            let remote = optional_text(v.get("remote"), 300)?;
            let row = MailMessage {
                id: id.clone(),
                owner_id: a.owner_id.clone(),
                account_id: a.id.clone(),
                mailbox_id: mailbox.id.clone(),
                remote,
                remote_key: String::new(),
                file_id,
                blob_id: blob,
                message_id: message_id.clone(),
                thread_key: format!(
                    "{}/{}",
                    a.owner_id,
                    if thread.is_empty() {
                        if message_id.is_empty() {
                            id.clone()
                        } else {
                            message_id
                        }
                    } else {
                        thread
                    }
                ),
                subject,
                from_name,
                from_address,
                recipients,
                date,
                snippet,
                seen: flag(v.get("seen"))?,
                flagged: false,
                answered: false,
                draft: flag(v.get("draft"))?,
                attachments,
                size,
                owner_key: String::new(),
                mailbox_key: String::new(),
                role_key: String::new(),
                unread_key: String::new(),
                flagged_key: String::new(),
            };
            ctx.db.mail_messages().insert(keyed(row, &mailbox.role));
            recount(ctx, &mailbox.id);
            Ok(json!({ "id": id }))
        }
        "finish" => {
            keys(p, &["accountId"])?;
            let a = account_of(p)?;
            let next = at + SYNC_MS;
            ctx.db.mail_accounts().id().update(MailAccount {
                status: "ready".into(),
                last_error: String::new(),
                error_count: 0,
                synced_at: at,
                next_sync: next,
                due_key: due_key(next),
                ..a
            });
            Ok(json!({ "ok": true }))
        }
        "fail" => {
            keys(p, &["accountId", "error", "signIn"])?;
            let a = account_of(p)?;
            let errors = a.error_count + 1;
            let delay = (SYNC_MS * 2f64.powi((errors - 1).min(5) as i32)).min(3_600_000.0);
            let next = at + delay;
            let status = if flag(p.get("signIn"))? {
                "signIn"
            } else {
                "error"
            };
            ctx.db.mail_accounts().id().update(MailAccount {
                status: status.into(),
                last_error: text(p.get("error"), 500, false)?,
                error_count: errors,
                next_sync: next,
                due_key: due_key(next),
                ..a
            });
            Ok(json!({ "ok": true }))
        }
        _ => fail("invalidInput"),
    }
}

/// Server mailboxes in, Files folders and mailbox rows out. `bound` gives folders made in Files
/// that the runtime just created on the server: `{folderId, remote}`.
fn store_mailboxes(
    ctx: &ReducerContext,
    a: &MailAccount,
    list: Option<&Value>,
    bound: Option<&Value>,
) -> Res<Value> {
    let list = list.and_then(Value::as_array).ok_or("invalidInput")?;
    if list.len() > MAILBOXES {
        return fail("operationTooLarge");
    }
    let mut bound_folders: HashMap<String, String> = HashMap::new();
    for b in bound.and_then(Value::as_array).cloned().unwrap_or_default() {
        let remote = text(b.get("remote"), 1000, false)?;
        // A mailbox renamed from Files keeps its row under its new server name.
        if let Some(id) = b.get("mailboxId").and_then(Value::as_str) {
            if let Some(m) = ctx
                .db
                .mail_mailboxes()
                .id()
                .find(id.to_owned())
                .filter(|m| m.account_id == a.id)
            {
                ctx.db
                    .mail_mailboxes()
                    .id()
                    .update(MailMailbox { remote, ..m });
            }
            continue;
        }
        bound_folders.insert(remote, text(b.get("folderId"), 100, false)?);
    }
    let existing: Vec<MailMailbox> = ctx.db.mail_mailboxes().account_id().filter(&a.id).collect();
    let mut by_remote: HashMap<String, MailMailbox> = existing
        .iter()
        .map(|m| (m.remote.clone(), m.clone()))
        .collect();
    let mut kept: HashSet<String> = HashSet::new();
    // Parents first, so each mailbox's folder can go inside its parent's.
    let mut pending: Vec<&Value> = list.iter().collect();
    let mut order = Vec::new();
    let mut placed: HashSet<String> = HashSet::new();
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|v| {
            let parent = v["parentRemote"].as_str().unwrap_or_default();
            if parent.is_empty()
                || placed.contains(parent)
                || !list.iter().any(|o| o["remote"].as_str() == Some(parent))
            {
                placed.insert(v["remote"].as_str().unwrap_or_default().to_owned());
                order.push(*v);
                false
            } else {
                true
            }
        });
        if pending.len() == before {
            order.extend(pending.drain(..));
        }
    }
    let root = account_folder(ctx, a)?;
    let a = ctx
        .db
        .mail_accounts()
        .id()
        .find(a.id.clone())
        .unwrap_or_else(|| a.clone());
    for v in order {
        let remote = text(v.get("remote"), 1000, false)?;
        let name = text(v.get("name"), 255, false)?;
        let role = optional_text(v.get("role"), 20)?;
        if !ROLES.contains(&role.as_str()) {
            return fail("invalidInput");
        }
        let parent_remote = optional_text(v.get("parentRemote"), 1000)?;
        let parent = (!parent_remote.is_empty())
            .then(|| by_remote.get(&parent_remote).cloned())
            .flatten();
        let parent_folder = match &parent {
            Some(p) => mailbox_folder(ctx, &a, p)?,
            None => root.clone(),
        };
        let validity = optional_text(v.get("validity"), 100)?;
        let row = match by_remote.get(&remote).cloned() {
            Some(old) => {
                let mut next = MailMailbox {
                    name: name.clone(),
                    role: role.clone(),
                    parent_id: parent.as_ref().map(|p| p.id.clone()).unwrap_or_default(),
                    ..old.clone()
                };
                if live_folder(ctx, &a.owner_id, &old.folder_id).is_none() {
                    next.folder_id = child_folder(ctx, &a.owner_id, &parent_folder, &name)?;
                } else if old.name != name {
                    // Renamed on the server: the folder follows when the name is free.
                    if let Some(f) = ctx.db.file_entries().id().find(old.folder_id.clone())
                        && let Ok(clean) = name_of(&name)
                        && find_key(ctx, &key(&a.owner_id, &f.parent_id, &clean)).is_none()
                    {
                        let _ = move_for(
                            ctx,
                            &a.owner_id,
                            &f.id,
                            &f.parent_id,
                            f.revision,
                            Some(&clean),
                            Unlock::Nobody,
                        );
                    }
                }
                if old.role != role {
                    // List keys carry the role.
                    for m in ctx
                        .db
                        .mail_messages()
                        .mailbox_id()
                        .filter(&old.id)
                        .collect::<Vec<_>>()
                    {
                        ctx.db.mail_messages().id().update(keyed(m, &role));
                    }
                }
                ctx.db.mail_mailboxes().id().update(next.clone());
                next
            }
            None => {
                let folder_id = match bound_folders
                    .get(&remote)
                    .filter(|f| live_folder(ctx, &a.owner_id, f).is_some())
                {
                    Some(f) => f.clone(),
                    None => child_folder(ctx, &a.owner_id, &parent_folder, &name)?,
                };
                let row = MailMailbox {
                    id: unused_uuid(ctx),
                    account_id: a.id.clone(),
                    owner_id: a.owner_id.clone(),
                    remote: remote.clone(),
                    name,
                    role,
                    parent_id: parent.as_ref().map(|p| p.id.clone()).unwrap_or_default(),
                    folder_id,
                    validity,
                    total: 0,
                    unread: 0,
                };
                ctx.db.mail_mailboxes().insert(row.clone());
                row
            }
        };
        kept.insert(row.id.clone());
        by_remote.insert(remote, row);
    }
    // Gone from the server: its messages leave Mail and their files move to deleted files.
    for old in existing.into_iter().filter(|m| !kept.contains(&m.id)) {
        for m in ctx
            .db
            .mail_messages()
            .mailbox_id()
            .filter(&old.id)
            .collect::<Vec<_>>()
        {
            trash_file(ctx, &a.owner_id, &m.file_id);
            ctx.db.mail_messages().id().delete(m.id);
        }
        ctx.db.mail_mailboxes().id().delete(old.id);
    }
    Ok(
        json!({ "mailboxes": ctx.db.mail_mailboxes().account_id().filter(&a.id).map(|m| json!({
        "id": m.id, "remote": m.remote, "name": m.name, "role": m.role, "parentId": m.parent_id, "validity": m.validity, "folderId": m.folder_id,
    })).collect::<Vec<_>>() }),
    )
}

/// What changed in the account's Files folder since the last pass, for the server to follow:
/// messages whose file was deleted, moved to another mailbox's folder, moved out of the mirror
/// or edited; `.eml` files added to a mailbox folder; folders made for new mailboxes; mailbox
/// folders renamed. A deleted mailbox folder (or account folder) is never taken as deleting its
/// messages: the folder is made again and its messages mirrored again.
fn plan(ctx: &ReducerContext, a: &MailAccount) -> Res<Value> {
    let owner = a.owner_id.as_str();
    let root_alive = live_folder(ctx, owner, &a.folder_id).is_some();
    let mailboxes: Vec<MailMailbox> = ctx.db.mail_mailboxes().account_id().filter(&a.id).collect();
    // Container mailboxes (IMAP \Noselect) cannot hold messages: files put there are left alone.
    let by_folder: HashMap<String, &MailMailbox> =
        mailboxes.iter().map(|m| (m.folder_id.clone(), m)).collect();
    let holds = |m: &&MailMailbox| m.role != "folder";
    let folder_alive = |m: &MailMailbox| -> bool {
        root_alive
            && live_folder(ctx, owner, &m.folder_id)
                .is_some_and(|f| matches!(place(ctx, &f, &a.folder_id), Place::Inside(_)))
    };
    let (mut gone, mut moved, mut changed, mut detached, mut remirror) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut fresh = NewFolders::default();
    for m in ctx.db.mail_messages().account_id().filter(&a.id) {
        if m.remote.is_empty() {
            continue;
        }
        let Some(mailbox) = mailboxes.iter().find(|b| b.id == m.mailbox_id) else {
            continue;
        };
        let file = ctx
            .db
            .file_entries()
            .id()
            .find(m.file_id.clone())
            .filter(|f| f.owner_id == owner);
        let entry = json!({ "id": m.id, "remote": m.remote, "mailboxId": m.mailbox_id });
        let Some(file) = file else {
            if folder_alive(mailbox) {
                gone.push(entry)
            } else {
                remirror.push(entry)
            }
            continue;
        };
        if !root_alive {
            remirror.push(entry);
            continue;
        }
        match place(ctx, &file, &a.folder_id) {
            Place::Trashed if file.trashed && folder_alive(mailbox) => gone.push(entry),
            Place::Trashed => remirror.push(entry),
            Place::Outside => detached.push(entry),
            Place::Inside(parent) => {
                if parent == mailbox.folder_id {
                    if file.blob_id != m.blob_id && !file.blob_id.is_empty() {
                        changed.push(json!({ "id": m.id, "remote": m.remote, "mailboxId": m.mailbox_id, "blobId": file.blob_id, "size": file.size }));
                    }
                } else if let Some(target) = by_folder.get(&parent) {
                    if !holds(target) {
                        continue;
                    }
                    moved.push(json!({ "id": m.id, "remote": m.remote, "mailboxId": m.mailbox_id, "targetMailboxId": target.id, "blobId": file.blob_id }));
                } else if let Some(folder) = fresh.add(ctx, a, &by_folder, &parent) {
                    moved.push(json!({ "id": m.id, "remote": m.remote, "mailboxId": m.mailbox_id, "targetFolderId": folder, "blobId": file.blob_id }));
                } else {
                    // Somewhere in the account folder that maps to no mailbox: left alone.
                }
            }
        }
    }
    // `.eml` files in mailbox folders (and new folders) that no message points at.
    let mut added = Vec::new();
    if root_alive {
        let mut folders: Vec<String> = vec![a.folder_id.clone()];
        let mut index = 0;
        while index < folders.len() && folders.len() < 2000 {
            let folder = folders[index].clone();
            index += 1;
            for child in crate::files::children(ctx, owner, &folder) {
                if child.trashed {
                    continue;
                }
                if child.kind == "folder" {
                    folders.push(child.id.clone());
                    if !by_folder.contains_key(&child.id) {
                        fresh.add(ctx, a, &by_folder, &child.id);
                    }
                    continue;
                }
                if folder == a.folder_id
                    || !child.name.to_lowercase().ends_with(".eml")
                    || child.blob_id.is_empty()
                {
                    continue;
                }
                if ctx
                    .db
                    .mail_messages()
                    .file_id()
                    .filter(&child.id)
                    .next()
                    .is_some()
                {
                    continue;
                }
                let target = match by_folder.get(&folder) {
                    Some(m) if !holds(m) => continue,
                    Some(m) => json!({ "mailboxId": m.id }),
                    None if fresh.ids.contains(&folder) => json!({ "targetFolderId": folder }),
                    None => continue,
                };
                let mut v = json!({ "fileId": child.id, "blobId": child.blob_id, "size": child.size, "modifiedAt": child.modified_at });
                v.as_object_mut()
                    .expect("object")
                    .extend(target.as_object().expect("object").clone());
                added.push(v);
                if added.len() >= 200 {
                    break;
                }
            }
        }
    }
    let renamed: Vec<Value> = mailboxes
        .iter()
        .filter_map(|m| {
            let f = live_folder(ctx, owner, &m.folder_id)?;
            (root_alive
                && f.name != m.name
                && name_of(&m.name).ok().as_deref() != Some(f.name.as_str())
                && m.role != "inbox")
                .then(|| json!({ "mailboxId": m.id, "name": f.name }))
        })
        .collect();
    Ok(json!({
        "gone": gone, "moved": moved, "changed": changed, "detached": detached, "remirror": remirror,
        "added": added, "newFolders": fresh.list, "renamed": renamed,
    }))
}

/// Folders in the account folder that are no mailbox's: each becomes a new mailbox.
#[derive(Default)]
struct NewFolders {
    ids: HashSet<String>,
    list: Vec<Value>,
}

impl NewFolders {
    fn add(
        &mut self,
        ctx: &ReducerContext,
        a: &MailAccount,
        by_folder: &HashMap<String, &MailMailbox>,
        folder_id: &str,
    ) -> Option<String> {
        if self.ids.contains(folder_id) {
            return Some(folder_id.to_owned());
        }
        let f = live_folder(ctx, &a.owner_id, folder_id)?;
        let parent = by_folder.get(&f.parent_id).map(|m| m.id.clone());
        if parent.is_none() && f.parent_id != a.folder_id && !self.ids.contains(&f.parent_id) {
            return None;
        }
        self.ids.insert(folder_id.to_owned());
        self.list.push(json!({ "folderId": f.id, "name": f.name, "parentMailboxId": parent.unwrap_or_default(), "parentFolderId": f.parent_id }));
        Some(folder_id.to_owned())
    }
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_mail(
    ctx: &mut spacetimedb::ProcedureContext,
    sessionId: String,
    operation: String,
    document: String,
) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let user = crate::from_session(tx, &sessionId)?;
        if tx
            .db
            .app_passwords()
            .session_id()
            .find(sessionId.clone())
            .is_some()
        {
            return fail("forbidden");
        }
        if document.len() > 2 * 1024 * 1024 {
            return fail("operationTooLarge");
        }
        mail_operation(tx, &user, &operation, &crate::document(&document)?)
    })
}

#[spacetimedb::procedure]
pub fn service_mail_worker(
    ctx: &mut spacetimedb::ProcedureContext,
    operation: String,
    document: String,
) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        if document.len() > 24 * 1024 * 1024 {
            return fail("operationTooLarge");
        }
        mail_worker(tx, &operation, &crate::document(&document)?)
    })
}
