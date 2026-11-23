//! The Notes library over Files (`runtime/notes/library.ts`): the module lists the Notes folder;
//! this reads each note version once from its immutable blob (cached by blob ID, so a cached
//! entry is never stale) for titles, excerpts, tags, aliases and links, and answers searches,
//! backlinks and unlinked mentions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::blobs::Blobs;
use crate::db::Database;
use crate::error::{AppError, Result};

const HEAD: u64 = 8192;
const FULL_LIMIT: u64 = 1024 * 1024;

#[derive(Default)]
pub struct Library {
    summaries: Mutex<(HashMap<String, Arc<Info>>, Vec<String>)>,
}

async fn read(blobs: &Blobs, blob_id: &str, bytes: u64) -> String {
    if blob_id.is_empty() {
        return String::new();
    }
    let Ok(path) = blobs.path(blob_id) else { return String::new() };
    let Ok(file) = tokio::fs::File::open(path).await else { return String::new() };
    let mut buffer = Vec::new();
    if file.take(bytes).read_to_end(&mut buffer).await.is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buffer).into_owned();
    text.strip_suffix('\u{fffd}').map(str::to_owned).unwrap_or(text)
}

/// `[[target|label]]` → label or target; `[text](url)` → text; images dropped.
fn plain_links(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("![")
            && let Some(close) = after.find(']')
            && let Some(inner) = after[close + 1..].strip_prefix('(')
            && let Some(end) = inner.find(')')
        {
            rest = &inner[end + 1..];
            continue;
        }
        // `![[embed]]` leaves the excerpt like an image.
        if let Some(after) = rest.strip_prefix("![[")
            && let Some(close) = after.find("]]")
        {
            rest = &after[close + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("[[")
            && let Some(close) = after.find("]]")
            && !after[..close].contains(']')
        {
            let inner = &after[..close];
            let (target, label) = inner.split_once('|').map_or((inner, None), |(t, l)| (t, Some(l)));
            if !target.is_empty() && !target.contains('|') {
                out.push_str(label.filter(|l| !l.is_empty()).unwrap_or_else(|| target.split('#').next().unwrap_or(target)));
                rest = &after[close + 2..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('[')
            && let Some(close) = after.find(']')
            && close > 0
            && let Some(inner) = after[close + 1..].strip_prefix('(')
            && let Some(end) = inner.find(')')
        {
            out.push_str(&after[..close]);
            rest = &inner[end + 1..];
            continue;
        }
        let c = rest.chars().next().expect("char");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn list_marker(line: &str) -> &str {
    // ^\s*(?:>\s*)?(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?
    let s = line.trim_start();
    let s2 = s.strip_prefix('>').map(str::trim_start).unwrap_or(s);
    let after = if let Some(r) = s2.strip_prefix(['-', '*', '+']) {
        Some(r)
    } else {
        let digits = s2.bytes().take_while(u8::is_ascii_digit).count();
        (digits > 0).then(|| s2[digits..].strip_prefix(['.', ')'])).flatten()
    };
    let Some(after) = after.filter(|a| a.starts_with(char::is_whitespace)) else { return line };
    let after = after.trim_start();
    for box_ in ["[ ]", "[x]", "[X]"] {
        if let Some(r) = after.strip_prefix(box_)
            && r.starts_with(char::is_whitespace)
        {
            return r.trim_start();
        }
    }
    after
}

fn heading_text(line: &str) -> Option<String> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) || !line[hashes..].starts_with(char::is_whitespace) {
        return None;
    }
    let text = line[hashes..].trim().trim_end_matches('#').trim_end();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Title from front matter or the first heading, else the file name; the excerpt is the first prose as plain text.
pub fn summarize(name: &str, text: &str) -> (String, String) {
    let mut body = text.strip_prefix('\u{feff}').unwrap_or(text).to_owned();
    let mut title = String::new();
    if let Some(rest) = body.strip_prefix("---\n").or_else(|| body.strip_prefix("---\r\n")) {
        let end = rest.find("\n---\n").map(|i| (i, i + 5)).or_else(|| rest.find("\r\n---\r\n").map(|i| (i, i + 7))).or_else(|| rest.strip_suffix("\n---").map(|r| (r.len(), rest.len())));
        if let Some((close, after)) = end {
            for line in rest[..close].lines() {
                if let Some(v) = line.strip_prefix("title:") {
                    title = v.trim().trim_matches(|c| c == '"' || c == '\'').to_owned();
                    break;
                }
            }
            body = rest[after..].to_owned();
        }
    }
    if title.is_empty() {
        for (index, line) in body.lines().enumerate() {
            if let Some(h) = heading_text(line) {
                let before: String = body.lines().take(index).collect::<Vec<_>>().join("\n");
                if before.trim().is_empty() {
                    title = h;
                }
                break;
            }
        }
    }
    let mut fenced = false;
    let joined = body
        .lines()
        .filter(|l| {
            // Code (and query blocks) stays out of the excerpt.
            let fence = l.trim_start().starts_with("```") || l.trim_start().starts_with("~~~");
            if fence {
                fenced = !fenced;
            }
            !fence && !fenced
        })
        .filter(|l| heading_text(l).is_none() && !(l.starts_with('#') && l.len() <= 7 && l.bytes().all(|b| b == b'#')))
        .map(list_marker)
        .collect::<Vec<_>>()
        .join(" ");
    let plain: String = plain_links(&joined).chars().filter(|c| !"*_`~>#".contains(*c)).collect();
    let excerpt: String = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let fallback = name.strip_suffix(".md").or_else(|| name.strip_suffix(".markdown")).or_else(|| name.strip_suffix(".MD")).unwrap_or(name);
    let title = if title.is_empty() { fallback.to_owned() } else { title };
    (utf16_prefix(&title, 200), utf16_prefix(&excerpt, 240))
}

fn utf16_prefix(value: &str, units: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(units).collect::<Vec<_>>())
}

/// What the library keeps of one note version (by blob ID, so it is never stale).
pub struct Info {
    title: String,
    excerpt: String,
    tags: Vec<String>,
    aliases: Vec<String>,
    links: Vec<LinkInfo>,
    properties: Value,
}

struct LinkInfo {
    target: String,
    wiki: bool,
    line: usize,
    text: String,
}

/// A line shown as the context of a link or mention: trimmed and bounded.
fn context_line(line: &str) -> String {
    utf16_prefix(line.trim(), 400)
}

fn info(name: &str, text: &str) -> Info {
    let (title, excerpt) = summarize(name, text);
    let facts = dar_notes::facts(text);
    let lines = dar_notes::Lines::new(text);
    let links = facts
        .links
        .iter()
        .map(|link| LinkInfo { target: link.target.clone(), wiki: link.wiki, line: link.line, text: context_line(&dar_notes::line_text(text, &lines, link.line)) })
        .collect();
    let properties: serde_json::Map<String, Value> = facts
        .properties
        .iter()
        .take(64)
        .map(|(k, v)| (k.clone(), match v { dar_notes::Prop::Text(t) => json!(utf16_prefix(t, 400)), dar_notes::Prop::List(items) => json!(items.iter().take(50).map(|i| utf16_prefix(i, 200)).collect::<Vec<_>>()) }))
        .collect();
    Info { title, excerpt, tags: facts.tags, aliases: facts.aliases, links, properties: Value::Object(properties) }
}

struct Note {
    row: Value,
    path: String,
    info: Arc<Info>,
}

impl Note {
    fn id(&self) -> &str {
        self.row["id"].as_str().unwrap_or_default()
    }
    fn heading(&self) -> Value {
        json!({ "id": self.row["id"], "title": self.info.title, "path": self.path, "revision": self.row["revision"] })
    }
}

impl Library {
    async fn info(&self, blobs: &Blobs, row: &Value) -> Arc<Info> {
        let blob_id = row["blobId"].as_str().unwrap_or_default().to_owned();
        if let Some(cached) = self.summaries.lock().expect("summaries").0.get(&blob_id) {
            return cached.clone();
        }
        let size = row["size"].as_f64().unwrap_or(0.0) as u64;
        let name = row["name"].as_str().unwrap_or_default();
        let text = read(blobs, &blob_id, if size <= FULL_LIMIT { size } else { HEAD.min(size) }).await;
        let value = Arc::new(info(name, &text));
        let mut cache = self.summaries.lock().expect("summaries");
        if cache.0.len() >= 20_000 {
            let oldest = cache.1.remove(0);
            cache.0.remove(&oldest);
        }
        cache.0.insert(blob_id.clone(), value.clone());
        cache.1.push(blob_id);
        value
    }

    async fn notes(&self, db: &Database, blobs: &Blobs, session_id: &str) -> Result<(Value, Vec<Note>)> {
        let listing = db.call("service_notes", json!([session_id, "library"])).await?;
        let mut notes = Vec::new();
        for row in listing["files"].as_array().into_iter().flatten() {
            let info = self.info(blobs, row).await;
            notes.push(Note { row: row.clone(), path: row["path"].as_str().unwrap_or_default().to_owned(), info });
        }
        Ok((listing, notes))
    }

    pub async fn library(&self, db: &Database, blobs: &Blobs, session_id: &str, query: &str) -> Result<Value> {
        let (listing, all) = self.notes(db, blobs, session_id).await?;
        let q = query.to_lowercase().trim().to_owned();
        let mut notes: Vec<Value> = Vec::new();
        for note in all {
            let (row, info) = (&note.row, &note.info);
            let name = row["name"].as_str().unwrap_or_default();
            let path = note.path.as_str();
            if !q.is_empty() && !format!("{name}\n{}\n{path}\n{}", info.title, info.aliases.join("\n")).to_lowercase().contains(&q) {
                // Search the whole note only when the name, heading and aliases do not match.
                let size = row["size"].as_f64().unwrap_or(0.0) as u64;
                let text = if size <= FULL_LIMIT { read(blobs, row["blobId"].as_str().unwrap_or_default(), size).await } else { String::new() };
                if !text.to_lowercase().contains(&q) {
                    continue;
                }
            }
            notes.push(json!({ "id": row["id"], "parentId": row["parentId"], "name": name, "title": info.title, "excerpt": info.excerpt, "size": row["size"], "revision": row["revision"], "modifiedAt": row["modifiedAt"], "favorite": row["favorite"], "path": path, "tags": info.tags, "aliases": info.aliases, "properties": info.properties }));
        }
        notes.sort_by(|a, b| {
            b["modifiedAt"].as_f64().unwrap_or(0.0).total_cmp(&a["modifiedAt"].as_f64().unwrap_or(0.0)).then_with(|| crate::http::locale_compare(a["title"].as_str().unwrap_or_default(), b["title"].as_str().unwrap_or_default()))
        });
        Ok(json!({ "rootId": listing["rootId"], "path": listing["path"], "folders": listing["folders"], "notes": notes, "truncated": listing["truncated"], "legacy": listing["legacy"] }))
    }

    /// Notes that link to `id`, each with the lines that hold the links.
    pub async fn backlinks(&self, db: &Database, blobs: &Blobs, session_id: &str, id: &str) -> Result<Value> {
        let (_, notes) = self.notes(db, blobs, session_id).await?;
        let vault = dar_notes::Vault::new(notes.iter().map(|n| n.path.clone()));
        let target = notes.iter().position(|n| n.id() == id).ok_or_else(AppError::not_found)?;
        let mut found = Vec::new();
        for (index, note) in notes.iter().enumerate() {
            if index == target {
                continue;
            }
            let mut contexts: Vec<Value> = Vec::new();
            for link in &note.info.links {
                let hit = if link.wiki { vault.resolve_wiki(&note.path, &link.target) } else { vault.resolve_path(&note.path, &link.target) };
                if hit == Some(target) && !contexts.iter().any(|c| c["line"] == json!(link.line)) && contexts.len() < 8 {
                    contexts.push(json!({ "line": link.line, "text": link.text }));
                }
            }
            if !contexts.is_empty() {
                let mut item = note.heading();
                item["contexts"] = json!(contexts);
                found.push(item);
            }
        }
        sort_by_title(&mut found);
        Ok(json!({ "id": id, "backlinks": found }))
    }

    /// Notes that name `id` (title, file name or alias) in plain text without linking to it.
    pub async fn mentions(&self, db: &Database, blobs: &Blobs, session_id: &str, id: &str) -> Result<Value> {
        let (_, notes) = self.notes(db, blobs, session_id).await?;
        let vault = dar_notes::Vault::new(notes.iter().map(|n| n.path.clone()));
        let target = notes.iter().position(|n| n.id() == id).ok_or_else(AppError::not_found)?;
        let stem = notes[target].path.rsplit('/').next().unwrap_or_default();
        let stem = stem.strip_suffix(".md").or_else(|| stem.strip_suffix(".markdown")).unwrap_or(stem);
        let mut names: Vec<String> = Vec::new();
        for name in std::iter::once(&notes[target].info.title).chain(std::iter::once(&stem.to_owned())).chain(notes[target].info.aliases.iter()) {
            if !names.iter().any(|n| n.to_lowercase() == name.to_lowercase()) {
                names.push(name.clone());
            }
        }
        // Longer names first, so "Project Apollo" wins over "Apollo".
        names.sort_by_key(|n| std::cmp::Reverse(n.chars().count()));
        let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let mut found = Vec::new();
        let mut truncated = false;
        for (index, note) in notes.iter().enumerate() {
            if index == target {
                continue;
            }
            let links_here = note.info.links.iter().any(|l| (if l.wiki { vault.resolve_wiki(&note.path, &l.target) } else { vault.resolve_path(&note.path, &l.target) }) == Some(target));
            if links_here {
                continue;
            }
            let size = note.row["size"].as_f64().unwrap_or(0.0) as u64;
            if size > FULL_LIMIT {
                continue;
            }
            let text = read(blobs, note.row["blobId"].as_str().unwrap_or_default(), size).await;
            let lower = text.to_lowercase();
            if !lowered.iter().any(|n| lower.contains(n.as_str())) {
                continue;
            }
            let facts = dar_notes::facts(&text);
            let lines = dar_notes::Lines::new(&text);
            let contexts: Vec<Value> = dar_notes::mentions(&text, &facts, &names)
                .into_iter()
                .take(8)
                .map(|r| {
                    let line = lines.line(r.start);
                    json!({ "line": line, "start": r.start, "end": r.end, "match": &text[r.clone()], "text": context_line(&dar_notes::line_text(&text, &lines, line)) })
                })
                .collect();
            if contexts.is_empty() {
                continue;
            }
            if found.len() >= 50 {
                truncated = true;
                break;
            }
            let mut item = note.heading();
            item["contexts"] = json!(contexts);
            found.push(item);
        }
        sort_by_title(&mut found);
        Ok(json!({ "id": id, "target": vault.wiki_target(target), "mentions": found, "truncated": truncated }))
    }
}

fn sort_by_title(list: &mut [Value]) {
    list.sort_by(|a, b| crate::http::locale_compare(a["title"].as_str().unwrap_or_default(), b["title"].as_str().unwrap_or_default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries() {
        assert_eq!(summarize("Plan.md", "---\ntitle: Weekly plan\ntags: [a]\n---\n# Ignored heading\n\n- [ ] **Buy** milk\n> [Link](https://x) and [[Page|alias]]"), ("Weekly plan".into(), "Buy milk Link and alias".into()));
        assert_eq!(summarize("Loose thoughts.md", "Just text, no heading."), ("Loose thoughts".into(), "Just text, no heading.".into()));
        assert_eq!(summarize("Plan.md", "---\ntitle: \"Trip\"\n---\nPack [[Bags|bags]] and [tickets](x).\n"), ("Trip".into(), "Pack bags and tickets.".into()));
        assert_eq!(summarize("Plan.md", "# Groceries\n- [ ] milk\n- [x] **bread**\n"), ("Groceries".into(), "milk bread".into()));
        assert_eq!(summarize("Plan.md", "Just text"), ("Plan".into(), "Just text".into()));
        assert_eq!(summarize("Shelf.md", "# Shelf\n```query\ntag: #b\n```\nAfter."), ("Shelf".into(), "After.".into()));
        assert_eq!(summarize("Day.md", "Read [[Book#Ch 2]].\n![[Book#Part]]\n![[dot.png]] done"), ("Day".into(), "Read Book. done".into()));
    }
}
