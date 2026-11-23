#![allow(non_snake_case)]
//! Rust port of Dar's SpacetimeDB module (`next/module/src`), file by file. Tables, reducers,
//! procedures and views match the TypeScript module by name, column and parameter (reducer
//! parameters keep their camelCase names), so either module reads the other's data. Procedures
//! return JSON strings; a failure aborts the transaction and reports Dar's error code.

pub mod agent;
pub mod app_settings;
pub mod auth;
pub mod backup;
pub mod blobs;
pub mod books;
pub mod browse;
pub mod chat;
pub mod files;
pub mod folders;
pub mod git;
pub mod global_search;
pub mod history;
pub mod identity;
pub mod immich;
pub mod mail;
pub mod news;
pub mod photo_sync;
pub mod photos;
pub mod photos_service;
pub mod pim;
pub mod scopes;
pub mod tables;
pub mod text_index;
pub mod tracker;
pub mod workspace;

use spacetimedb::{ProcedureContext, ReducerContext, Table, ViewContext};
use unicode_normalization::UnicodeNormalization;

use tables::*;

#[allow(non_snake_case)]
#[spacetimedb::env]
pub struct Env {
    /// The browser token issuer (`<origin>/auth/issuer`).
    pub DAR_ISSUER: String,
}

pub type Res<T> = Result<T, String>;

pub fn fail<T>(code: &str) -> Res<T> {
    Err(code.to_owned())
}

pub fn now(ctx: &ReducerContext) -> f64 {
    (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as f64
}

/// Only the runtime's service identity may call `service_*` operations.
pub fn service(ctx: &ReducerContext) -> Res<()> {
    match ctx.db.instance().id().find("instance".to_owned()) {
        Some(row) if row.service == ctx.sender() => Ok(()),
        _ => fail("forbidden"),
    }
}

/// Run a procedure body in one transaction. On failure the transaction rolls back and the
/// procedure fails with the code (see `abort`).
pub fn run(ctx: &mut ProcedureContext, body: impl Fn(&ReducerContext) -> Res<String>) -> String {
    match ctx.try_with_tx(|tx| body(tx)) {
        Ok(value) => value,
        Err(code) => abort::with(&code),
    }
}

/// Like `run`, but returns `{ "error": { "code": ... } }` instead of trapping (`readResult` in `index.ts`).
pub fn read_result(ctx: &mut ProcedureContext, body: impl Fn(&ReducerContext) -> Res<String>) -> String {
    match ctx.try_with_tx(|tx| body(tx)) {
        Ok(value) => value,
        Err(code) => serde_json::json!({ "error": { "code": code } }).to_string(),
    }
}

/// The account behind a live session.
pub fn from_session(ctx: &ReducerContext, id: &str) -> Res<Account> {
    let session = ctx.db.sessions().id().find(id.to_owned());
    let Some(session) = session.filter(|s| s.active && s.expires_at > now(ctx)) else {
        return fail("unauthenticated");
    };
    match ctx.db.accounts().id().find(session.user_id) {
        Some(user) if user.enabled => Ok(user),
        _ => fail("unauthenticated"),
    }
}

/// The account behind the calling principal (a browser tab or an app password).
pub fn active_user(ctx: &ReducerContext) -> Res<Account> {
    let principal = ctx.db.principals().identity().find(ctx.sender()).ok_or("unauthenticated")?;
    from_session(ctx, &principal.session_id)
}

/// Views have no clock; there scheduled expiry deactivates the session instead.
pub fn user_for_view(ctx: &ViewContext) -> Option<Account> {
    let principal = ctx.db.principals().identity().find(ctx.sender())?;
    let session = ctx.db.sessions().id().find(principal.session_id).filter(|s| s.active)?;
    ctx.db.accounts().id().find(session.user_id).filter(|u| u.enabled)
}

/// A file or folder name: NFC, trimmed, at most 255 UTF-16 units, no controls or separators.
pub fn name_of(name: &str) -> Res<String> {
    let name: String = name.nfc().collect::<String>().trim().to_owned();
    let bad = name.is_empty() || name.encode_utf16().count() > 255 || name == "." || name == ".." || name.chars().any(|c| (c as u32) < 0x20 || c == '/' || c == '\\');
    if bad { fail("invalidName") } else { Ok(name) }
}

/// A lowercase UUID, as the runtime generates for new records.
pub fn opaque_id(value: &str) -> Res<()> {
    let groups: Vec<&str> = value.split('-').collect();
    let shape = [8, 4, 4, 4, 12];
    let ok = groups.len() == 5 && groups.iter().zip(shape).all(|(g, n)| g.len() == n && g.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    if ok { Ok(()) } else { fail("invalidInput") }
}

/// A fresh UUID not used by any record the module names.
pub fn unused_uuid(ctx: &ReducerContext) -> String {
    loop {
        let id = ctx.new_uuid_v4().expect("reducer randomness").to_string();
        let taken = ctx.db.activity().id().find(id.clone()).is_some()
            || ctx.db.file_entries().id().find(id.clone()).is_some()
            || ctx.db.versions().id().find(id.clone()).is_some()
            || ctx.db.sessions().id().find(id.clone()).is_some()
            || ctx.db.uploads().id().find(id.clone()).is_some()
            || ctx.db.git_comments().id().find(id.clone()).is_some()
            || ctx.db.git_labels().id().find(id.clone()).is_some()
            || ctx.db.principals().identity().find(identity_of(&id)).is_some();
        if !taken {
            return id;
        }
    }
}

/// `[prefix, prefix+1)`, or after `cursor` within it: the keys that start with `prefix`.
pub fn window_of(prefix: &str, cursor: Option<&str>) -> (std::ops::Bound<String>, std::ops::Bound<String>) {
    use std::ops::Bound;
    let mut points: Vec<char> = prefix.chars().collect();
    let mut upper = String::new();
    while let Some(last) = points.pop() {
        let code = last as u32;
        if code == 0x10ffff {
            continue;
        }
        let next = char::from_u32(if code == 0xd7ff { 0xe000 } else { code + 1 }).expect("valid scalar");
        upper = points.iter().collect::<String>();
        upper.push(next);
        break;
    }
    let lower = match cursor {
        Some(c) => Bound::Excluded(c.to_owned()),
        None => Bound::Included(prefix.to_owned()),
    };
    (lower, if upper.is_empty() { Bound::Unbounded } else { Bound::Excluded(upper) })
}

#[spacetimedb::reducer(init)]
pub fn init(ctx: &ReducerContext) {
    ctx.db.instance().insert(Instance { id: "instance".into(), service: ctx.sender(), issuer: ctx.env.DAR_ISSUER() });
}

#[cfg(test)]
mod tests;

/// A failed procedure answers as a TypeScript one that throws: with an HTTP error whose text
/// names the code, which the runtime (`crates/runtime/src/db.rs`) looks for. A Rust
/// procedure can only fail by trapping, and the host reports a trap with its backtrace, not its
/// message; so each code traps from a function named after it, which the backtrace shows. The
/// transaction has rolled back before this runs.
mod abort {
    macro_rules! codes {
        ($($code:ident),*) => {
            $(#[allow(non_snake_case)] #[inline(never)] #[cold] pub fn $code() -> ! { panic!(stringify!($code)) })*
            pub fn with(code: &str) -> ! {
                match code {
                    $(stringify!($code) => $code(),)*
                    _ => internalError(),
                }
            }
        };
    }
    codes!(lockedFolder, nothingToCommit, uncommittedChanges, conflict, alreadyExists, notFound, forbidden, unauthenticated, invalidInput, invalidName,
        quotaExceeded, operationTooLarge, contentUnavailable, internalError);
}

/// JSON as `JSON.stringify` writes it: integral numbers without a fraction (`1`, not `1.0`).
pub fn stringify(value: &serde_json::Value) -> String {
    fn plain(value: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match value {
            Value::Number(n) => match n.as_f64() {
                Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 => Value::from(f as i64),
                _ => value.clone(),
            },
            Value::Array(items) => Value::Array(items.iter().map(plain).collect()),
            Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), plain(v))).collect()),
            other => other.clone(),
        }
    }
    plain(value).to_string()
}

/// `run` for bodies that build JSON.
pub fn run_json(ctx: &mut ProcedureContext, body: impl Fn(&ReducerContext) -> Res<serde_json::Value>) -> String {
    run(ctx, |tx| body(tx).map(|v| stringify(&v)))
}

/// A procedure's JSON document: an object, or `invalidInput` (`JSON.parse` then the shape check).
pub fn document(text: &str) -> Res<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        _ => fail("invalidInput"),
    }
}

/// A row as JSON with its TypeScript field names.
pub fn row<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("row JSON")
}

/// `String(value)` for the common cases a document carries (missing is "undefined", as in TypeScript).
pub fn text_of(value: Option<&serde_json::Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) => "null".into(),
        Some(serde_json::Value::Number(n)) => n.as_f64().map(|f| if f.fract() == 0.0 && f.abs() < 1e21 { format!("{}", f as i64) } else { format!("{f}") }).unwrap_or_default(),
        Some(other) => other.to_string(),
    }
}

/// A key prefix's index window (`prefix` as `Range` bounds).
pub fn prefix(value: &str) -> (std::ops::Bound<String>, std::ops::Bound<String>) {
    window_of(value, None)
}

/// `new Identity(BigInt("0x" + uuid without dashes))`: the UUID as a number, little-endian.
pub fn identity_of(uuid: &str) -> spacetimedb::Identity {
    let hex: String = uuid.chars().filter(|c| *c != '-').collect();
    let value = u128::from_str_radix(&hex, 16).unwrap_or(0);
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(&value.to_le_bytes());
    spacetimedb::Identity::from_byte_array(bytes)
}

/// Index range bounds as borrowed strings, for `filter`.
pub fn bounds(range: &(std::ops::Bound<String>, std::ops::Bound<String>)) -> (std::ops::Bound<&str>, std::ops::Bound<&str>) {
    (range.0.as_ref().map(String::as_str), range.1.as_ref().map(String::as_str))
}

/// `[prefix, prefix with its last character's successor)` as `prefix.slice(0,-1)+"0"` builds it
/// for keys ending in "/" (the TypeScript module's folder windows).
pub fn folder_window(prefix: &str) -> (std::ops::Bound<String>, std::ops::Bound<String>) {
    let upper = format!("{}0", &prefix[..prefix.len() - 1]);
    (std::ops::Bound::Included(prefix.to_owned()), std::ops::Bound::Excluded(upper))
}

/// `a.localeCompare(b)` (ICU root collation) for the strings Dar sorts: letters by their base
/// form ignoring case and accents, then lowercase before uppercase, then code points.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    use unicode_normalization::char::is_combining_mark;
    let base = |s: &str| -> String { s.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect() };
    let key = |s: &str| -> Vec<(u8, char)> {
        // Punctuation and spaces sort before digits, digits before letters, as in ICU.
        base(s).chars().map(|c| (if c.is_alphabetic() { 2 } else if c.is_numeric() { 1 } else { 0 }, c)).collect()
    };
    key(a).cmp(&key(b)).then_with(|| {
        let case = |s: &str| -> Vec<bool> { s.chars().map(char::is_uppercase).collect() };
        case(a).cmp(&case(b))
    }).then_with(|| a.cmp(b))
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian (as `Date` in UTC).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// `new Date(ms).toISOString()`.
pub fn iso_string(ms: f64) -> String {
    let ms = ms.floor() as i64;
    let (days, rest) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let (y, m, d) = civil_from_days(days);
    let (h, min, s, milli) = (rest / 3_600_000, rest / 60_000 % 60, rest / 1000 % 60, rest % 1000);
    let year = if (0..=9999).contains(&y) { format!("{y:04}") } else if y < 0 { format!("-{:06}", -y) } else { format!("+{y:06}") };
    format!("{year}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.{milli:03}Z")
}

/// `Math.round`: halves up.
pub fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}
