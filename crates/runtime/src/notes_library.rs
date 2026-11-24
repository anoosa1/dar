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
        // A trailing `^block` id is an anchor, not text.
        .map(|l| match dar_notes::block_id(l) { Some(id) => l.trim_end()[..l.trim_end().len() - id.len() - 1].trim_end(), None => l })
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
    cards: Vec<CardInfo>,
}

struct LinkInfo {
    target: String,
    wiki: bool,
    line: usize,
    text: String,
}

struct CardInfo {
    index: usize,
    question: String,
    answer: String,
    due: Option<dar_notes::cards::Date>,
    interval: Option<u32>,
    ease: Option<u32>,
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
    let cards = dar_notes::cards::find(text)
        .into_iter()
        .map(|card| CardInfo {
            index: card.index,
            question: card.question,
            answer: card.answer,
            due: card.schedule.map(|s| s.due),
            interval: card.schedule.map(|s| s.interval),
            ease: card.schedule.map(|s| s.ease),
        })
        .collect();
    Info { title, excerpt, tags: facts.tags, aliases: facts.aliases, links, properties: Value::Object(properties), cards }
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

    /// Global search rows for the notes a `#tag` query or an alias names, which only this cache
    /// knows (the module's search matches file names and text).
    pub async fn search(&self, db: &Database, blobs: &Blobs, session_id: &str, query: &str) -> Result<Vec<Value>> {
        let q = query.trim();
        let term = q.strip_prefix('#').unwrap_or(q).trim();
        if term.is_empty() || (!q.starts_with('#') && term.chars().count() < 2) {
            return Ok(Vec::new());
        }
        let (_, notes) = self.notes(db, blobs, session_id).await?;
        Ok(search_hits(&notes, q))
    }

    /// Nodes and deduplicated resolved link edges across the whole Notes library.
    pub async fn graph(&self, db: &Database, blobs: &Blobs, session_id: &str) -> Result<Value> {
        let (_, notes) = self.notes(db, blobs, session_id).await?;
        Ok(build_graph(&notes))
    }

    /// Due and new flashcards across the whole Notes library, served from cached per-blob `Info`.
    pub async fn cards(&self, db: &Database, blobs: &Blobs, session_id: &str, today: Option<dar_notes::cards::Date>) -> Result<Value> {
        let (_, notes) = self.notes(db, blobs, session_id).await?;
        Ok(build_cards(&notes, today.unwrap_or_else(today_date)))
    }
}

fn today_date() -> dar_notes::cards::Date {
    use chrono::Datelike;
    let now = chrono::Local::now().date_naive();
    dar_notes::cards::Date::new(now.year(), now.month() as u8, now.day() as u8).expect("local date")
}

struct DeckTotals {
    due: usize,
    new: usize,
    total: usize,
}

fn build_cards(notes: &[Note], today: dar_notes::cards::Date) -> Value {
    let mut due_count = 0usize;
    let mut new_count = 0usize;
    let mut total_count = 0usize;
    let mut deck_map: HashMap<String, DeckTotals> = HashMap::new();
    let mut items: Vec<(Option<dar_notes::cards::Date>, String, String, usize, Value)> = Vec::new();

    for note in notes {
        if note.info.cards.is_empty() {
            continue;
        }
        let deck_name = dar_notes::cards::deck(&note.path, &note.info.tags);
        let entry = deck_map.entry(deck_name.clone()).or_insert(DeckTotals { due: 0, new: 0, total: 0 });
        let revision = note.row["revision"]
            .as_u64()
            .or_else(|| note.row["revision"].as_f64().map(|n| n as u64))
            .unwrap_or(0);
        for card in &note.info.cards {
            total_count += 1;
            entry.total += 1;
            let include = match card.due {
                Some(due) if due <= today => {
                    due_count += 1;
                    entry.due += 1;
                    true
                }
                Some(_) => false,
                None => {
                    new_count += 1;
                    entry.new += 1;
                    true
                }
            };
            if include {
                let payload = json!({
                    "noteId": note.id(),
                    "title": note.info.title,
                    "path": note.path,
                    "revision": revision,
                    "cardIndex": card.index,
                    "question": card.question,
                    "answer": card.answer,
                    "due": card.due.map(|d| d.to_string()),
                    "interval": card.interval,
                    "ease": card.ease,
                    "deck": deck_name,
                });
                items.push((card.due, note.info.title.clone(), note.path.clone(), card.index, payload));
            }
        }
    }

    items.sort_by(|a, b| {
        let due_ord = match (a.0, b.0) {
            (Some(da), Some(db)) => da.cmp(&db),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        due_ord
            .then_with(|| crate::http::locale_compare(&a.1, &b.1))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.cmp(&b.3))
    });

    let mut decks: Vec<(String, DeckTotals)> = deck_map.into_iter().collect();
    decks.sort_by(|a, b| crate::http::locale_compare(&a.0, &b.0));
    let decks: Vec<Value> = decks
        .into_iter()
        .map(|(name, counts)| json!({ "name": name, "due": counts.due, "new": counts.new, "total": counts.total }))
        .collect();
    let cards: Vec<Value> = items.into_iter().map(|(_, _, _, _, v)| v).collect();

    json!({
        "today": today.to_string(),
        "due": due_count,
        "new": new_count,
        "total": total_count,
        "decks": decks,
        "cards": cards,
    })
}

/// Notes with a tag starting with `#query`'s tag (nested tags included), or an alias containing
/// `query`, as global search rows (Notes group, at most eight, by title).
fn search_hits(notes: &[Note], query: &str) -> Vec<Value> {
    let q = query.trim().to_lowercase();
    let (by_tag, term) = match q.strip_prefix('#') {
        Some(tag) => (true, tag.trim().to_owned()),
        None => (false, q.clone()),
    };
    let mut hits: Vec<Value> = Vec::new();
    for note in notes {
        let detail = if by_tag {
            if !note.info.tags.iter().any(|t| t.to_lowercase().starts_with(&term)) {
                continue;
            }
            note.info.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")
        } else {
            match note.info.aliases.iter().find(|a| a.to_lowercase().contains(&term)) {
                Some(alias) => format!("Alias: {alias}"),
                None => continue,
            }
        };
        hits.push(json!({ "id": note.id(), "group": "Notes", "title": note.info.title, "detail": utf16_prefix(&detail, 240), "href": format!("/notes?note={}", note.id()) }));
    }
    sort_by_title(&mut hits);
    hits.truncate(8);
    hits
}

/// Adds `hits` to the module's search result after its own Notes rows, without duplicates and
/// within the group's eight rows (more set `truncated`).
pub fn merge_search(result: &mut Value, hits: Vec<Value>) {
    let Some(items) = result["items"].as_array_mut() else { return };
    let mut count = items.iter().filter(|i| i["group"] == "Notes").count();
    let mut truncated = false;
    for hit in hits {
        if items.iter().any(|i| i["group"] == "Notes" && i["id"] == hit["id"]) {
            continue;
        }
        if count >= 8 {
            truncated = true;
            break;
        }
        items.push(hit);
        count += 1;
    }
    if truncated {
        result["truncated"] = json!(true);
    }
}

fn build_graph(notes: &[Note]) -> Value {
    let vault = dar_notes::Vault::new(notes.iter().map(|n| n.path.clone()));
    let mut nodes: Vec<Value> = notes
        .iter()
        .map(|n| json!({ "id": n.id(), "title": n.info.title, "path": n.path, "tags": n.info.tags }))
        .collect();
    sort_by_title(&mut nodes);
    let mut seen = std::collections::HashSet::new();
    let mut edges: Vec<Value> = Vec::new();
    for (index, note) in notes.iter().enumerate() {
        let from_id = note.id();
        if from_id.is_empty() {
            continue;
        }
        for link in &note.info.links {
            let hit = if link.wiki {
                vault.resolve_wiki(&note.path, &link.target)
            } else {
                vault.resolve_path(&note.path, &link.target)
            };
            if let Some(target) = hit
                && target != index
                && seen.insert((index, target))
            {
                let to_id = notes[target].id();
                if !to_id.is_empty() {
                    edges.push(json!({ "from": from_id, "to": to_id }));
                }
            }
        }
    }
    json!({ "nodes": nodes, "edges": edges })
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
        assert_eq!(summarize("B.md", "- one ^a1\ntwo ^b-2"), ("B".into(), "one two".into()));
        assert_eq!(summarize("Shelf.md", "# Shelf\n```query\ntag: #b\n```\nAfter."), ("Shelf".into(), "After.".into()));
        assert_eq!(summarize("Day.md", "Read [[Book#Ch 2]].\n![[Book#Part]]\n![[dot.png]] done"), ("Day".into(), "Read Book. done".into()));
    }

    #[test]
    fn graph_edges() {
        let make = |id: &str, path: &str, text: &str| {
            let name = path.rsplit('/').next().unwrap_or(path);
            Note { row: json!({ "id": id, "name": name }), path: path.to_owned(), info: Arc::new(info(name, text)) }
        };
        let notes = vec![
            make("id-a", "A.md", "# Alpha\nSee [[B]], [again](B.md), [[A]], [[#Alpha]], and [[Missing]]. #work\n"),
            make("id-b", "B.md", "# Beta\nNo outgoing links.\n"),
            make("id-c", "sub/C.md", "Back to [[A]] and [[B]].\n"),
        ];
        let g = build_graph(&notes);
        let nodes = g["nodes"].as_array().expect("nodes");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0], json!({ "id": "id-a", "title": "Alpha", "path": "A.md", "tags": ["work"] }));
        assert_eq!(nodes[1], json!({ "id": "id-b", "title": "Beta", "path": "B.md", "tags": [] }));
        assert_eq!(nodes[2], json!({ "id": "id-c", "title": "C", "path": "sub/C.md", "tags": [] }));
        let edges = g["edges"].as_array().expect("edges");
        assert_eq!(edges, &[
            json!({ "from": "id-a", "to": "id-b" }),
            json!({ "from": "id-c", "to": "id-a" }),
            json!({ "from": "id-c", "to": "id-b" }),
        ]);
    }

    #[test]
    fn cards_decks_and_due_filter() {
        let make = |id: &str, path: &str, revision: u64, text: &str| {
            let name = path.rsplit('/').next().unwrap_or(path);
            Note { row: json!({ "id": id, "name": name, "revision": revision }), path: path.to_owned(), info: Arc::new(info(name, text)) }
        };
        let notes = vec![
            make(
                "n1",
                "Languages/Spanish.md",
                3,
                "---\ntags: [flashcards/vocab]\n---\n# Spanish\nHola::Hello\n<!--SR:!2026-11-20,4,250-->\n\nAdios::Bye\n<!--SR:!2026-12-01,10,250-->\n",
            ),
            make(
                "n2",
                "Science/Bio.md",
                2,
                "# Bio\nCell powerhouse?\n?\nMitochondria\n",
            ),
            make(
                "n3",
                "Root.md",
                1,
                "# Root\n- Logseq Q #card\n  - Logseq A\n",
            ),
            make("n4", "Plain.md", 1, "# Plain\nNo cards here.\n"),
        ];
        let today = dar_notes::cards::Date::parse("2026-11-23").expect("today");
        let out = build_cards(&notes, today);
        assert_eq!(out["today"], "2026-11-23");
        assert_eq!(out["due"], 1);
        assert_eq!(out["new"], 2);
        assert_eq!(out["total"], 4);

        let decks = out["decks"].as_array().expect("decks");
        assert_eq!(decks, &[
            json!({ "name": "Notes", "due": 0, "new": 1, "total": 1 }),
            json!({ "name": "Science", "due": 0, "new": 1, "total": 1 }),
            json!({ "name": "vocab", "due": 1, "new": 0, "total": 2 }),
        ]);

        let cards = out["cards"].as_array().expect("cards");
        assert_eq!(cards.len(), 3);
        // Due card first, then new cards sorted by title.
        assert_eq!(cards[0], json!({
            "noteId": "n1",
            "title": "Spanish",
            "path": "Languages/Spanish.md",
            "revision": 3,
            "cardIndex": 0,
            "question": "Hola",
            "answer": "Hello",
            "due": "2026-11-20",
            "interval": 4,
            "ease": 250,
            "deck": "vocab",
        }));
        assert_eq!(cards[1]["noteId"], "n2");
        assert_eq!(cards[1]["deck"], "Science");
        assert_eq!(cards[1]["due"], Value::Null);
        assert_eq!(cards[2]["noteId"], "n3");
        assert_eq!(cards[2]["deck"], "Notes");
        assert_eq!(cards[2]["answer"], "- Logseq A");
    }

    #[test]
    fn tag_and_alias_search() {
        let make = |id: &str, name: &str, text: &str| Note { row: json!({ "id": id, "name": name }), path: name.to_owned(), info: Arc::new(info(name, text)) };
        let notes = vec![
            make("a", "Dune.md", "---\ntags: [book/scifi]\naliases: [Arrakis]\n---\n# Dune\n"),
            make("b", "Emma.md", "# Emma\nA #book and #classic.\n"),
            make("c", "Plan.md", "# Plan\n#work\n"),
        ];
        let ids = |hits: &Vec<Value>| hits.iter().map(|h| h["id"].as_str().unwrap_or_default().to_owned()).collect::<Vec<_>>();
        let hits = search_hits(&notes, "#Book");
        assert_eq!(ids(&hits), ["a", "b"]);
        assert_eq!(hits[1], json!({ "id": "b", "group": "Notes", "title": "Emma", "detail": "#book #classic", "href": "/notes?note=b" }));
        assert_eq!(ids(&search_hits(&notes, "arrak")), ["a"]);
        assert_eq!(search_hits(&notes, "arrak")[0]["detail"], "Alias: Arrakis");
        assert!(search_hits(&notes, "#nothing").is_empty());

        let mut result = json!({ "items": [{ "id": "b", "group": "Notes" }, { "id": "f", "group": "Files" }], "truncated": false });
        merge_search(&mut result, search_hits(&notes, "#book"));
        assert_eq!(ids(result["items"].as_array().unwrap()), ["b", "f", "a"]);
        assert_eq!(result["truncated"], false);
        let mut full = json!({ "items": (0..8).map(|i| json!({ "id": i.to_string(), "group": "Notes" })).collect::<Vec<_>>(), "truncated": false });
        merge_search(&mut full, search_hits(&notes, "#book"));
        assert_eq!(full["items"].as_array().unwrap().len(), 8);
        assert_eq!(full["truncated"], true);
    }
}
