//! Chat runtime (`next/runtime/chat/*`): supervises the `dar-chat-bridge` process over stdio
//! NDJSON, encrypts bridge keys with `KeyBox`, downloads attachments into the blob store,
//! manages live Discord voice WebSockets, and serves the `/api/v1/chat/*` HTTP API.

mod bridge;
mod http;
mod live;
mod media;
mod runtime;

pub use http::handle_http;
pub use runtime::ChatRuntime;

use chrono::{DateTime, Datelike, Local, Timelike};

/// Chat database operations that only read (`next/shared/read-operations.ts`).
pub(crate) const CHAT_READ_OPERATIONS: &[&str] = &[
    "access",
    "overview",
    "threads",
    "messages",
    "account",
    "media",
    "target",
    "chatTarget",
    "chatInfo",
    "oldest",
    "names",
    "contactSearch",
    "kvLoad",
    "kvGet",
    "accounts",
    "mediaFind",
    "servers",
    "find",
    "newest",
];

pub(crate) const CHAT_LIMIT_ACCOUNTS: usize = 10;
pub(crate) const CHAT_LIMIT_NAME: usize = 64;
pub(crate) const CHAT_LIMIT_TEXT: usize = 65_536;
pub(crate) const CHAT_LIMIT_DISCORD_TEXT: usize = 4_000;

/// Current Unix timestamp in milliseconds.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64
}

/// Matches `/^[0-9a-f-]{36}$/` as used for Chat account IDs in `runtime.ts` and `http.ts`.
pub(crate) fn is_account_id(value: &str) -> bool {
    value.len() == 36
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

/// Converts a JSON value to an `i64` (handling both integer and float representations from SpacetimeDB).
pub(crate) fn json_i64(value: Option<&serde_json::Value>) -> i64 {
    value
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(0)
}

/// Converts a JSON value to a string matching JS `String(v ?? "")`.
pub(crate) fn value_to_string(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// The name an attachment downloads and saves under (`localName` in `next/shared/chat.ts`).
pub(crate) fn local_name(
    kind: &str,
    mime: &str,
    file_name: &str,
    at_ms: f64,
    platform: &str,
) -> String {
    let cleaned: String = file_name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let given = cleaned.trim();
    if !given.is_empty() && given != "." && given != ".." {
        return given.chars().take(200).collect();
    }
    let dt = DateTime::from_timestamp_millis(at_ms.trunc() as i64)
        .unwrap_or(DateTime::UNIX_EPOCH)
        .with_timezone(&Local);
    let platform_label = if platform == "discord" {
        "Discord"
    } else {
        "WhatsApp"
    };
    let kind_label = match kind {
        "image" => "Photo",
        "sticker" => "Sticker",
        "video" => "Video",
        "audio" => "Audio",
        "voice" => "Voice message",
        "document" => "Document",
        _ => "Attachment",
    };
    let ext = match mime {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "video/mp4" => Some("mp4"),
        "video/3gpp" => Some("3gp"),
        "audio/ogg" => Some("ogg"),
        "audio/mpeg" => Some("mp3"),
        "audio/mp4" => Some("m4a"),
        "audio/aac" => Some("aac"),
        "application/pdf" => Some("pdf"),
        _ => None,
    };
    let suffix = ext.map(|e| format!(".{e}")).unwrap_or_default();
    format!(
        "{platform_label} {kind_label} {:04}-{:02}-{:02} at {:02}.{:02}.{:02}{suffix}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
    )
}
