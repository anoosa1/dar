//! The text of stored notes and text files, for search (`text-index.ts`). It belongs to the
//! blob, the runtime fills it, and it goes when the blob does; not backed up.

use serde_json::{Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::blobs::is_blob_id;
use crate::tables::*;
use crate::{Res, fail};

pub const TEXT_INDEX_BYTES: f64 = 2.0 * 1024.0 * 1024.0;
pub const TEXT_INDEX_CHARS: usize = 256 * 1024;

const TEXT_EXTENSIONS: [&str; 34] = [
    "md", "markdown", "txt", "text", "csv", "tsv", "json", "yml", "yaml", "toml", "ini", "cfg",
    "conf", "log", "xml", "htm", "html", "css", "js", "ts", "tsx", "jsx", "py", "go", "rs", "sh",
    "sql", "tex", "org", "rst", "adoc", "vcf", "ics", "markdown",
];

/// `isTextFile` (shared/search.ts).
pub fn is_text_file(name: &str, media_type: &str) -> bool {
    let typed = [
        "json",
        "xml",
        "x-yaml",
        "yaml",
        "toml",
        "javascript",
        "x-sh",
        "sql",
    ]
    .iter()
    .any(|t| media_type == format!("application/{t}"));
    let extension = name.rsplit_once('.').map(|(_, e)| e.to_lowercase());
    media_type.starts_with("text/")
        || typed
        || extension.is_some_and(|e| TEXT_EXTENSIONS.contains(&e.as_str()))
}

pub fn blobs_without_text(ctx: &ReducerContext, after: &str, limit: u32) -> Res<Value> {
    if !(1..=200).contains(&limit) {
        return fail("invalidInput");
    }
    let mut ids: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    for entry in ctx.db.file_entries().iter() {
        if entry.kind != "file"
            || entry.trashed
            || !is_blob_id(&entry.blob_id)
            || entry.blob_id.as_str() <= after
            || entry.size > TEXT_INDEX_BYTES
        {
            continue;
        }
        if !is_text_file(&entry.name, &entry.media_type)
            || ctx
                .db
                .blob_texts()
                .id()
                .find(entry.blob_id.clone())
                .is_some()
        {
            continue;
        }
        ids.insert(entry.blob_id, entry.size);
    }
    Ok(Value::Array(
        ids.into_iter()
            .take(limit as usize)
            .map(|(id, size)| json!({ "id": id, "size": size }))
            .collect(),
    ))
}

pub fn record_texts(ctx: &ReducerContext, items: &Value) -> Res<()> {
    let Some(items) = items.as_array().filter(|a| a.len() <= 200) else {
        return fail("invalidInput");
    };
    for item in items {
        let (Some(id), Some(text)) = (item["id"].as_str(), item["text"].as_str()) else {
            return fail("invalidInput");
        };
        if !is_blob_id(id) || text.encode_utf16().count() > TEXT_INDEX_CHARS {
            return fail("invalidInput");
        }
        // Content collected while it was read gets no row.
        if ctx.db.file_blobs().id().find(id.to_owned()).is_some()
            && ctx.db.blob_texts().id().find(id.to_owned()).is_none()
        {
            ctx.db.blob_texts().insert(BlobText {
                id: id.into(),
                text: text.into(),
            });
        }
    }
    Ok(())
}

/// A short excerpt around the first match, for a search result's detail line. Offsets are
/// UTF-16 units, as JavaScript counts them.
pub fn excerpt(text: &str, q: &str) -> String {
    let normalized: String = unicode_normalization::UnicodeNormalization::nfc(text)
        .collect::<String>()
        .to_lowercase();
    let units: Vec<u16> = normalized.encode_utf16().collect();
    let needle: Vec<u16> = q.encode_utf16().collect();
    let Some(at) = (if needle.is_empty() {
        Some(0)
    } else {
        units
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
    }) else {
        return String::new();
    };
    let source: Vec<u16> = text.encode_utf16().collect();
    let start = at.saturating_sub(50);
    let end = (at + needle.len() + 70).min(source.len());
    let slice = String::from_utf16_lossy(&source[start.min(source.len())..end]);
    let collapsed = slice.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        collapsed,
        if end < source.len() { "…" } else { "" }
    )
}

#[spacetimedb::procedure]
pub fn service_blobs_without_text(
    ctx: &mut spacetimedb::ProcedureContext,
    after: String,
    limit: u32,
) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        blobs_without_text(tx, &after, limit)
    })
}

#[spacetimedb::reducer]
pub fn service_record_texts(ctx: &ReducerContext, items: String) -> Res<()> {
    crate::service(ctx)?;
    record_texts(
        ctx,
        &serde_json::from_str(&items).map_err(|_| "invalidInput".to_owned())?,
    )
}
