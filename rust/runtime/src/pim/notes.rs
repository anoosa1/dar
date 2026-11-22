//! Notes as Markdown with a small projection (`runtime/pim/notes.ts`).

use serde_json::{Value, json};

use super::vobject::{field, invalid};
use crate::error::Result;

/// A note's Markdown and projection, from form input and the previous projection (`readNote`).
pub fn read_note(input: &Value, previous: Option<&Value>) -> Result<(String, Value)> {
    let p = input.as_object().ok_or_else(|| invalid("Invalid note."))?;
    let content = match p.get("content").and_then(Value::as_str) {
        Some(c) if c.len() <= 262_144 && !c.contains('\0') => c.to_owned(),
        _ => return Err(invalid("A note must contain at most 256 KiB of Markdown.")),
    };
    let pick = |key: &str| p.get(key).filter(|v| !v.is_null()).or_else(|| previous.and_then(|v| v.get(key)).filter(|v| !v.is_null())).cloned();
    let title = field(&pick("title").unwrap_or(json!("Untitled note")), 255)?.trim().to_owned();
    if title.is_empty() {
        return Err(invalid("A note needs a title."));
    }
    let tags = pick("tags").unwrap_or(json!([]));
    let tags = tags.as_array().filter(|t| t.len() <= 50).ok_or_else(|| invalid("Use at most 50 tags."))?;
    let mut unique: Vec<String> = Vec::new();
    for tag in tags {
        let tag = field(tag, 100)?.trim().to_owned();
        if !tag.is_empty() && !unique.contains(&tag) {
            unique.push(tag);
        }
    }
    let favorite = match p.get("favorite") {
        None | Some(Value::Null) => previous.and_then(|v| v["favorite"].as_bool()).unwrap_or(false),
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(invalid("Invalid favorite value.")),
    };
    Ok((content, json!({ "kind": "note", "title": title, "tags": unique, "favorite": favorite })))
}
