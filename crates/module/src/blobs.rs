//! Content-addressed blobs (`next/module/src/blobs.ts`). A blob's name is the SHA-256 of its
//! bytes; versions and backup pins keep it.

use spacetimedb::{ReducerContext, Table};

use crate::tables::{FileBlob, file_blobs};
use crate::{Res, fail, now};

/// Grace before maintenance checks new content (`blobGraceMs`).
pub const BLOB_GRACE_MS: f64 = 2.0 * 3_600_000.0;

pub fn is_blob_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `checkKey`: when maintenance should next look at the blob ("" for never).
pub fn check_key_at(time: f64) -> String {
    format!("{:016}", time.max(0.0).floor() as u64)
}

/// Record that the runtime stored this content and something may use it.
pub fn store_blob(ctx: &ReducerContext, id: &str, size: f64) -> Res<()> {
    if !is_blob_id(id) || size < 0.0 || size.fract() != 0.0 {
        return fail("invalidInput");
    }
    let later = check_key_at(now(ctx) + BLOB_GRACE_MS);
    match ctx.db.file_blobs().id().find(id.to_owned()) {
        None => {
            ctx.db.file_blobs().insert(FileBlob { id: id.into(), size, state: "stored".into(), check_key: later });
        }
        Some(row) if row.size != size => return fail("conflict"),
        Some(row) if row.state != "stored" => {
            ctx.db.file_blobs().id().update(FileBlob { state: "stored".into(), check_key: later, ..row });
        }
        Some(_) => {}
    }
    Ok(())
}

/// Something using this content went away; maintenance checks whether anything still does.
pub fn release_blob(ctx: &ReducerContext, id: &str) {
    let due = check_key_at(now(ctx));
    if let Some(row) = ctx.db.file_blobs().id().find(id.to_owned())
        && row.state == "stored"
        && (row.check_key.is_empty() || row.check_key > due)
    {
        ctx.db.file_blobs().id().update(FileBlob { check_key: due, ..row });
    }
}
