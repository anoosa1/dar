//! Dar's Photos runtime, Public Photo Links, Immich mobile-app API, and Immich server connector
//! (`next/runtime/photos/*`).

pub mod http;
pub mod immich;
pub mod immich_connector;
pub mod media;
pub mod public;
pub mod runtime;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64_STD;

pub use http::handle_http;
pub use immich::{IMMICH_BASE, handle_immich, immich_well_known};
pub use immich_connector::ImmichConnector;
pub use public::handle_public;
pub use runtime::PhotosRuntime;

/// Dar's per-file size limit for Files, WebDAV and Immich uploads and imports (`shared/limits.ts`).
pub(crate) const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub(crate) const MAX_FILE_LABEL: &str = "16 GiB";

/// Password-protected shared-link pass lifetime in days (`shared/photos.ts`).
pub(crate) const PASSWORD_DAYS: f64 = 0.5;

/// Database operations that only read (`shared/read-operations.ts`).
pub(crate) const PHOTOS_READ_OPERATIONS: &[&str] = &[
    "overview",
    "page",
    "detail",
    "asset",
    "duplicates",
    "stackDetail",
    "albumSharing",
];

/// Dar's built-in OIDC client for the Immich mobile app (`shared/photos.ts`).
pub(crate) const IMMICH_OAUTH_CLIENT_ID: &str = "immich-app";
pub(crate) const IMMICH_OAUTH_REDIRECT_URI: &str = "app.immich:///oauth-callback";

pub(crate) const RAW_EXTENSIONS: &[&str] = &["dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf"];

/// Media types Photos indexes (`photoKind` in `shared/photos.ts`).
pub(crate) fn photo_kind(media_type: &str, name: &str) -> Option<&'static str> {
    let t = media_type.to_ascii_lowercase();
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if t.starts_with("image/") && t != "image/svg+xml" {
        return Some("image");
    }
    if t.starts_with("video/") {
        return Some("video");
    }
    if [
        "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", "tif", "tiff",
    ]
    .contains(&ext.as_str())
        || RAW_EXTENSIONS.contains(&ext.as_str())
    {
        return Some("image");
    }
    if ["mp4", "mov", "m4v", "webm", "mkv", "3gp", "avi"].contains(&ext.as_str()) {
        return Some("video");
    }
    None
}

/// Splits a file name into its stem and lower-case extension (`nameParts` in `shared/photos.ts`).
pub(crate) fn name_parts(name: &str) -> (&str, String) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], name[dot + 1..].to_ascii_lowercase()),
        _ => (name, String::new()),
    }
}

/// Shared-link passwords: 4–200 characters (`validLinkPassword` in `shared/photos.ts`).
pub(crate) fn valid_link_password(value: &str) -> bool {
    value.chars().count() >= 4 && value.encode_utf16().count() <= 200 && value.len() <= 1024
}

pub(crate) fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as f64
}

/// Encodes a 40-char hex SHA-1 digest as standard base64, as Immich clients send checksums.
pub(crate) fn sha1_hex_to_base64(hex: &str) -> String {
    if hex.len() != 40 {
        return String::new();
    }
    let mut bytes = [0u8; 20];
    for (i, slot) in bytes.iter_mut().enumerate() {
        let Ok(b) = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16) else {
            return String::new();
        };
        *slot = b;
    }
    B64_STD.encode(bytes)
}

/// Parses an ISO 8601 / RFC 3339 date string into epoch milliseconds, like JS `Date.parse`.
pub(crate) fn parse_iso_ms(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.timestamp_millis() as f64);
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(ndt.and_utc().timestamp_millis() as f64);
    }
    if let Ok(nd) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        && let Some(ndt) = nd.and_hms_opt(0, 0, 0)
    {
        return Some(ndt.and_utc().timestamp_millis() as f64);
    }
    None
}

/// Extracts a cookie value by name from the `Cookie` header.
pub(crate) fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let cookies = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    cookies
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(name)?.strip_prefix('=').map(str::to_owned))
}

/// Formats epoch milliseconds as an ISO-8601 UTC timestamp with millisecond precision (`Date.toISOString`).
pub(crate) fn iso_ms(ms: f64) -> String {
    let clamped = if ms.is_finite() && ms > 0.0 { ms.trunc() as i64 } else { 0 };
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(clamped)
        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(0).expect("epoch"))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
