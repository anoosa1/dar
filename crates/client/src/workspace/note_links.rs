//! Links between notes: the library's link index, `[[` and `#` suggestions while writing,
//! linked and unlinked references below a note (as Logseq shows them), and link updates after
//! a note is renamed or moved (as Obsidian makes them). Resolution follows `dar-notes`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dar_notes::{Link, Vault};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use super::notes::{NotesFile, NotesLibrary, fetch_detail, has_md_ext, read_file_content};
use crate::api::{self, BrowserSession};
use crate::files::editor::{TextFile, upload_blob, write_text_file};
use crate::live::{Live, reducer, replied};
use crate::module_bindings::create_folder;
use crate::markdown::{Embed, Markdown, NoteLinks, note_embed, split_fragment};
use crate::ui::{Listener, call_method, document, listen, window};

/// A file in the Notes folder that is not a note (an image, PDF or recording).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteFile {
    pub id: String,
    pub parent_id: String,
    pub name: String,
    #[serde(default)]
    pub media_type: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct NoteFiles {
    #[serde(default)]
    pub files: Vec<NoteFile>,
}

/// The whole library, for resolving links (the list on screen may be a search result).
pub struct NoteIndex {
    pub vault: Vault,
    pub notes: Vec<NotesFile>,
    /// Tags with the number of notes carrying them, by name.
    pub tags: Vec<(String, usize)>,
    pub files: Vec<NoteFile>,
    pub file_vault: Vault,
    pub root_id: String,
    /// The `attachments` folder pasted and dropped files go to, once it exists.
    pub attachments: Option<String>,
}

impl NoteIndex {
    /// What an embed in `source_id` shows: a note, another file, or nothing.
    pub fn embed(&self, source_id: &str, target: &str, wiki: bool) -> Option<Embed> {
        if wiki && dar_notes::is_note_target(target) {
            return self.resolve(source_id, target, true).map(|n| Embed::Note { id: n.id.clone() });
        }
        let source = self.path_of(source_id).unwrap_or_default();
        let hit = if wiki { self.file_vault.resolve_name(source, target) } else { self.file_vault.resolve_path(source, target) };
        hit.map(|i| {
            let file = &self.files[i];
            Embed::File { id: file.id.clone(), name: file.name.clone(), media_type: file.media_type.clone() }
        })
    }

    pub fn with_files(mut self, files: Option<&NoteFiles>) -> Self {
        self.files = files.map(|f| f.files.clone()).unwrap_or_default();
        self.file_vault = Vault::new(self.files.iter().map(|f| f.path.clone()));
        self
    }

    pub fn new(library: Option<&NotesLibrary>) -> Self {
        let notes: Vec<NotesFile> = library.map(|l| l.notes.clone()).unwrap_or_default();
        let vault = Vault::new(notes.iter().map(|n| n.path.clone()));
        let mut tags: Vec<(String, usize)> = Vec::new();
        for note in &notes {
            for tag in &note.tags {
                match tags.iter_mut().find(|(t, _)| t.to_lowercase() == tag.to_lowercase()) {
                    Some(entry) => entry.1 += 1,
                    None => tags.push((tag.clone(), 1)),
                }
            }
        }
        tags.sort_by_key(|(t, _)| t.to_lowercase());
        let root_id = library.map(|l| l.root_id.clone()).unwrap_or_default();
        let attachments = library.and_then(|l| l.folders.iter().find(|f| f.path.eq_ignore_ascii_case("attachments")).map(|f| f.id.clone()));
        Self { vault, notes, tags, files: Vec::new(), file_vault: Vault::new(Vec::<String>::new()), root_id, attachments }
    }

    pub fn position(&self, id: &str) -> Option<usize> {
        self.notes.iter().position(|n| n.id == id)
    }

    pub fn path_of(&self, id: &str) -> Option<&str> {
        self.notes.iter().find(|n| n.id == id).map(|n| n.path.as_str())
    }

    /// The note a link in `source_id` names.
    pub fn resolve(&self, source_id: &str, target: &str, wiki: bool) -> Option<&NotesFile> {
        let source = self.path_of(source_id).unwrap_or_default();
        let hit = if wiki { self.vault.resolve_wiki(source, target) } else { self.vault.resolve_path(source, target) };
        hit.map(|i| &self.notes[i])
    }
}

/// True when `note` carries `tag` or a tag nested below it (`project` covers `project/dar`).
pub fn has_tag(note: &NotesFile, tag: &str) -> bool {
    let tag = tag.to_lowercase();
    note.tags.iter().any(|t| {
        let t = t.to_lowercase();
        t == tag || t.starts_with(&format!("{tag}/"))
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    /// `[[query` before the caret; `start` is the byte offset of `[[`.
    Link { start: usize, query: String },
    /// `#query` before the caret; `start` is the byte offset of `#`.
    Tag { start: usize, query: String },
    /// `/query` after a space or at a line start: commands and templates.
    Slash { start: usize, query: String },
}

/// Where the caret goes in an inserted command (removed on insert).
pub const CARET: char = '\u{1}';

/// Notes in the `Templates` folder, inserted with `{{date}}`, `{{time}}` and `{{title}}` filled in.
pub fn is_template(note: &NotesFile) -> bool {
    note.path.to_lowercase().starts_with("templates/")
}

/// `{{date}}`, `{{time}}` and `{{title}}` (and `{{date:YYYY-MM-DD}}`-style formats Obsidian
/// writes, read as the plain date) filled in; template properties are dropped.
pub fn fill_template(template: &str, title: &str, date: &str, time: &str) -> String {
    let body = dar_notes::front_matter(template).map_or(template, |(_, end)| &template[end..]);
    let mut out = String::new();
    let mut rest = body;
    while let Some(open) = rest.find("{{") {
        let Some(close) = rest[open..].find("}}") else { break };
        out.push_str(&rest[..open]);
        let name = rest[open + 2..open + close].trim().to_lowercase();
        match name.split(':').next().unwrap_or_default() {
            "date" => out.push_str(date),
            "time" => out.push_str(time),
            "title" => out.push_str(title),
            _ => out.push_str(&rest[open..open + close + 2]),
        }
        rest = &rest[open + close + 2..];
    }
    out.push_str(rest);
    out
}

/// Today's date (`YYYY-MM-DD`) and time (`HH:MM`) in the browser's timezone.
pub fn today() -> (String, String) {
    let now = js_sys::Date::new_0();
    (format!("{:04}-{:02}-{:02}", now.get_full_year(), now.get_month() + 1, now.get_date()), format!("{:02}:{:02}", now.get_hours(), now.get_minutes()))
}

fn commands(date: &str, time: &str) -> Vec<(&'static str, &'static str, String)> {
    vec![
        ("Heading 1", "Big section heading", format!("# {CARET}")),
        ("Heading 2", "Medium section heading", format!("## {CARET}")),
        ("Heading 3", "Small section heading", format!("### {CARET}")),
        ("To-do", "Checkbox", format!("- [ ] {CARET}")),
        ("Bulleted list", "", format!("- {CARET}")),
        ("Numbered list", "", format!("1. {CARET}")),
        ("Quote", "", format!("> {CARET}")),
        ("Callout", "Note, tip, warning\u{2026}", format!("> [!note] {CARET}\n> ")),
        ("Code block", "", format!("```\n{CARET}\n```")),
        ("Table", "Two columns", format!("| {CARET} |  |\n| --- | --- |\n|  |  |")),
        ("Divider", "", format!("---\n{CARET}")),
        ("Link to note", "[[", format!("[[{CARET}]]")),
        ("Date", "Today", date.to_owned()),
        ("Time", "Now", time.to_owned()),
        ("Math", "Formula", format!("$${CARET}$$")),
    ]
}

/// What the text before the caret (a byte offset) asks to complete.
pub fn trigger(text: &str, caret: usize) -> Option<Trigger> {
    let caret = caret.min(text.len());
    let line_start = text[..caret].rfind('\n').map_or(0, |i| i + 1);
    let line = &text[line_start..caret];
    if let Some(open) = line.rfind("[[") {
        let query = &line[open + 2..];
        if !query.contains("]]") && !query.contains(['|', '[', '#']) && query.chars().count() <= 80 {
            return Some(Trigger::Link { start: line_start + open, query: query.to_owned() });
        }
    }
    if let Some(slash) = line.rfind('/') {
        let query = &line[slash + 1..];
        let before = line[..slash].chars().next_back();
        if before.is_none_or(char::is_whitespace) && query.len() <= 24 && query.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return Some(Trigger::Slash { start: line_start + slash, query: query.to_owned() });
        }
    }
    let word_len: usize = line.chars().rev().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/')).map(char::len_utf8).sum();
    let hash = caret - word_len;
    if hash > line_start && text[..hash].ends_with('#') {
        let at = hash - 1;
        let before = text[..at].chars().next_back();
        let after = text[caret..].chars().next();
        if before.is_none_or(char::is_whitespace) && !after.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return Some(Trigger::Tag { start: at, query: text[hash..caret].to_owned() });
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub detail: String,
    pub insert: String,
    /// A template note to fetch and fill in instead of `insert`.
    pub template: Option<String>,
}

fn rank(name: &str, query: &str) -> Option<u8> {
    let name = name.to_lowercase();
    if query.is_empty() || name.starts_with(query) {
        Some(0)
    } else if name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(query)) {
        Some(1)
    } else if name.contains(query) {
        Some(2)
    } else {
        None
    }
}

/// Up to eight completions: notes for a link (by title, file name or alias), tags for a tag.
pub fn suggestions(index: &NoteIndex, trigger: &Trigger, source_id: &str) -> Vec<Suggestion> {
    match trigger {
        Trigger::Link { query, .. } => {
            let q = query.trim().to_lowercase();
            let mut found: Vec<(u8, f64, Suggestion)> = Vec::new();
            for (i, note) in index.notes.iter().enumerate() {
                if note.id == source_id {
                    continue;
                }
                let target = index.vault.wiki_target(i);
                let stem = target.rsplit('/').next().unwrap_or(&target).to_owned();
                let folder = note.path.rfind('/').map(|i| note.path[..i].to_owned()).unwrap_or_default();
                let by_name = rank(&note.title, &q).into_iter().chain(rank(&stem, &q)).min();
                let by_alias = note.aliases.iter().filter_map(|a| rank(a, &q).map(|r| (r, a))).min_by_key(|(r, _)| *r);
                let (score, insert, label) = match (by_name, by_alias) {
                    (Some(n), Some((a, alias))) if a < n => (a, format!("[[{target}|{alias}]]"), alias.clone()),
                    (Some(n), _) => (n, format!("[[{target}]]"), note.title.clone()),
                    (None, Some((a, alias))) => (a, format!("[[{target}|{alias}]]"), alias.clone()),
                    (None, None) => continue,
                };
                let detail = if label != note.title { format!("{} \u{b7} {}", note.title, folder) } else { folder };
                found.push((score, note.modified_at, Suggestion { label, detail: detail.trim_end_matches([' ', '\u{b7}']).to_owned(), insert, template: None }));
            }
            found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
            found.into_iter().take(8).map(|(_, _, s)| s).collect()
        }
        Trigger::Tag { query, .. } => {
            let q = query.to_lowercase();
            let mut found: Vec<(u8, usize, &str)> = index.tags.iter().filter(|(t, _)| t.to_lowercase() != q).filter_map(|(t, n)| rank(t, &q).map(|r| (r, *n, t.as_str()))).collect();
            found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(b.2)));
            found
                .into_iter()
                .take(8)
                .map(|(_, n, t)| Suggestion { label: format!("#{t}"), detail: format!("{n} {}", if n == 1 { "note" } else { "notes" }), insert: format!("#{t} "), template: None })
                .collect()
        }
        Trigger::Slash { query, .. } => {
            let q = query.to_lowercase();
            let (date, time) = today();
            let mut found: Vec<(u8, Suggestion)> = commands(&date, &time)
                .into_iter()
                .filter_map(|(label, detail, insert)| rank(label, &q).map(|r| (r, Suggestion { label: label.into(), detail: detail.into(), insert, template: None })))
                .collect();
            for note in index.notes.iter().filter(|n| is_template(n)) {
                let name = super::notes::strip_md_ext(&note.name);
                let label = format!("Template: {name}");
                if let Some(r) = rank(&name, &q).or_else(|| rank(&label, &q)) {
                    found.push((r, Suggestion { label, detail: "Insert template".into(), insert: String::new(), template: Some(note.id.clone()) }));
                }
            }
            found.sort_by_key(|(r, _)| *r);
            found.into_iter().take(10).map(|(_, s)| s).collect()
        }
    }
}

/// The text with a suggestion accepted at `caret` (bytes), and the caret after it.
pub fn accept(text: &str, caret: usize, trigger: &Trigger, insert: &str) -> (String, usize) {
    let (start, mut end) = match trigger {
        Trigger::Link { start, .. } | Trigger::Tag { start, .. } | Trigger::Slash { start, .. } => (*start, caret),
    };
    if matches!(trigger, Trigger::Link { .. }) && text[end..].starts_with("]]") {
        end += 2;
    }
    if matches!(trigger, Trigger::Tag { .. }) && text[end..].starts_with(' ') {
        end += 1;
    }
    let at = insert.find(CARET).unwrap_or(insert.len());
    let insert = insert.replace(CARET, "");
    let next = format!("{}{insert}{}", &text[..start], &text[end..]);
    (next, start + at)
}

/// The viewport position just below the caret of a textarea (a mirror element measures it).
pub fn caret_point(area: &web_sys::HtmlTextAreaElement, utf16: u32) -> Option<(f64, f64)> {
    let style = window().get_computed_style(area).ok().flatten()?;
    let mirror = document().create_element("div").ok()?.dyn_into::<web_sys::HtmlElement>().ok()?;
    let copy = [
        "box-sizing", "width", "padding-top", "padding-right", "padding-bottom", "padding-left", "border-top-width", "border-right-width", "border-bottom-width",
        "border-left-width", "font-family", "font-size", "font-weight", "font-style", "letter-spacing", "line-height", "text-transform", "text-indent", "tab-size", "word-spacing",
    ];
    let target = mirror.style();
    for name in copy {
        let _ = target.set_property(name, &style.get_property_value(name).unwrap_or_default());
    }
    for (name, value) in [("position", "fixed"), ("visibility", "hidden"), ("white-space", "pre-wrap"), ("overflow-wrap", "break-word"), ("top", "0"), ("left", "0"), ("border-style", "solid")] {
        let _ = target.set_property(name, value);
    }
    let value = area.value();
    let before: String = String::from_utf16_lossy(&value.encode_utf16().take(utf16 as usize).collect::<Vec<_>>());
    mirror.set_text_content(Some(&before));
    let marker = document().create_element("span").ok()?.dyn_into::<web_sys::HtmlElement>().ok()?;
    marker.set_text_content(Some("\u{200b}"));
    mirror.append_child(&marker).ok()?;
    document().body()?.append_child(&mirror).ok()?;
    let (x, y, h) = (marker.offset_left() as f64, marker.offset_top() as f64, marker.offset_height() as f64);
    mirror.remove();
    let rect = area.get_bounding_client_rect();
    Some((rect.left() + x - area.scroll_left() as f64, rect.top() + y - area.scroll_top() as f64 + h + 4.0))
}

/// The suggestion list next to the caret (top layer, so the sheet does not clip it).
#[component]
pub fn LinkSuggestions(
    id: String,
    #[prop(into)] items: Signal<Vec<Suggestion>>,
    #[prop(into)] active: Signal<usize>,
    #[prop(into)] point: Signal<Option<(f64, f64)>>,
    on_pick: Callback<usize>,
) -> impl IntoView {
    let panel = NodeRef::<html::Div>::new();
    let prefix = id.clone();
    Effect::new(move |_| {
        let (Some(element), Some((x, y))) = (panel.get(), point.get()) else { return };
        let element: web_sys::HtmlElement = element.unchecked_into();
        let open = !items.with(Vec::is_empty);
        let shown = element.matches(":popover-open").unwrap_or(false);
        if open {
            let width = window().inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
            let style = element.style();
            let _ = style.set_property("--float-top", &format!("{}px", y.round()));
            let _ = style.set_property("--float-left", &format!("{}px", x.min(width - 300.0).max(8.0).round()));
            if !shown {
                call_method(&element, "showPopover");
            }
        } else if shown {
            call_method(&element, "hidePopover");
        }
    });
    view! {
        <div node_ref=panel id=id popover="manual" class="note-suggest" role="listbox" aria-label="Suggestions">
            {move || items.get().into_iter().enumerate().map(|(i, item)| {
                let prefix = prefix.clone();
                let option_id = format!("{prefix}-{i}");
                view! {
                    <div role="option" id=option_id class="note-suggest-option" aria-selected=move || (active.get() == i).to_string()
                        on:mousedown=move |e| { e.prevent_default(); on_pick.try_run(i); }>
                        <span>{item.label.clone()}</span>
                        {(!item.detail.is_empty()).then(|| view! { <small>{item.detail.clone()}</small> })}
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct RefContext {
    line: usize,
    text: String,
    #[serde(default)]
    start: usize,
    #[serde(default)]
    end: usize,
    #[serde(default, rename = "match")]
    matched: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct RefNote {
    id: String,
    title: String,
    path: String,
    contexts: Vec<RefContext>,
}

#[derive(Deserialize)]
struct Backlinks {
    backlinks: Vec<RefNote>,
}

#[derive(Clone, Deserialize)]
struct Mentions {
    target: String,
    mentions: Vec<RefNote>,
    #[serde(default)]
    truncated: bool,
}

/// IDs of the notes linking to `id` (empty when they cannot be listed).
pub async fn backlink_ids(id: &str) -> Vec<String> {
    match api::get::<Backlinks>(&format!("/api/v1/notes/backlinks?id={}", js_sys::encode_uri_component(id))).await {
        Ok(list) => list.backlinks.into_iter().map(|n| n.id).collect(),
        Err(_) => Vec::new(),
    }
}

fn folder_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// After `moved` took `new_path`, points the links in `sources` (and the moved note's own
/// relative links) at it again, each write checked against the revision read. Returns how many
/// notes changed.
pub async fn retarget_links(live: Live, session: &BrowserSession, before: &NoteIndex, moves: &[(String, String)], sources: Vec<String>) -> Result<usize, String> {
    let moved: std::collections::HashMap<usize, &str> = moves.iter().filter_map(|(id, path)| before.position(id).map(|i| (i, path.as_str()))).collect();
    if moved.is_empty() {
        return Ok(0);
    }
    let after = Vault::new(before.vault.paths().iter().enumerate().map(|(i, p)| moved.get(&i).map_or_else(|| p.clone(), |n| (*n).to_owned())));
    let mut ids = sources;
    for (id, _) in moves {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    let mut changed = 0;
    let mut failed = 0;
    for id in ids {
        let Some(old_source) = before.path_of(&id).map(str::to_owned) else { continue };
        let source_index = before.position(&id);
        let new_source = source_index.and_then(|i| moved.get(&i)).map_or_else(|| old_source.clone(), |p| (*p).to_owned());
        let source_moved = folder_of(&old_source) != folder_of(&new_source);
        let conn = live.connection.get_untracked().ok_or("Notes are reconnecting. Try again in a moment.")?;
        let Ok(detail) = fetch_detail(&conn, &id).await else {
            failed += 1;
            continue;
        };
        if !has_md_ext(&detail.entry.name) {
            continue;
        }
        let Ok(text) = read_file_content(&id).await else {
            failed += 1;
            continue;
        };
        let next = dar_notes::rewrite(&text, |link: &Link| {
            let hit = before.vault.resolve(&old_source, link)?;
            if link.wiki && link.target.trim().is_empty() {
                return None;
            }
            if moved.contains_key(&hit) || (source_moved && !link.wiki) {
                return Some(after.retarget(&new_source, link, hit));
            }
            None
        });
        let Some(next) = next else { continue };
        let entry = &detail.entry;
        let file = TextFile { name: &entry.name, parent_id: &entry.parent_id, content: &next, media_type: &entry.media_type, file_id: Some(&entry.id), expected_revision: Some(entry.revision) };
        match write_text_file(session, file).await {
            Ok(_) => changed += 1,
            Err(_) => failed += 1,
        }
    }
    if failed > 0 {
        return Err(format!("Links in {failed} {} could not be updated. Check them for the old name.", if failed == 1 { "note" } else { "notes" }));
    }
    Ok(changed)
}

/// Linked and unlinked references at the end of a note. Unlinked ones load when opened and
/// can be turned into links.
#[component]
pub fn NoteReferences(
    #[prop(into)] note_id: Signal<String>,
    #[prop(into)] version: Signal<u32>,
    links: NoteLinks,
    session: BrowserSession,
    on_open: Callback<String>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let live = expect_context::<Live>();
    let backlinks: RwSignal<Option<Vec<RefNote>>> = RwSignal::new(None);
    let mentions: RwSignal<Option<Mentions>> = RwSignal::new(None);
    let mentions_open = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let session = StoredValue::new(session);
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }
    let seq = RwSignal::new(0u32);

    let alive_load = alive.clone();
    Effect::new(move |_| {
        let id = note_id.get();
        version.track();
        let want_mentions = mentions_open.get();
        let ticket = seq.get_untracked() + 1;
        seq.set(ticket);
        let alive = alive_load.clone();
        spawn_local(async move {
            let encoded = js_sys::encode_uri_component(&id);
            let linked = api::get::<Backlinks>(&format!("/api/v1/notes/backlinks?id={encoded}")).await;
            let unlinked = if want_mentions { Some(api::get::<Mentions>(&format!("/api/v1/notes/mentions?id={encoded}")).await) } else { None };
            if !alive.load(Ordering::Relaxed) || seq.try_get_untracked() != Some(ticket) {
                return;
            }
            match linked {
                Ok(list) => {
                    backlinks.set(Some(list.backlinks));
                    error.set(String::new());
                }
                Err(e) if e.status != 404 => error.set("References could not be loaded.".into()),
                Err(_) => backlinks.set(Some(Vec::new())),
            }
            if let Some(result) = unlinked {
                match result {
                    Ok(list) => mentions.set(Some(list)),
                    Err(_) => error.set("Unlinked references could not be loaded.".into()),
                }
            }
        });
    });

    let link_mention = move |note: RefNote, context: RefContext, target: String| {
        let alive = alive.clone();
        spawn_local(async move {
            busy.set(true);
            let result: Result<(), String> = async {
                let conn = live.connection.get_untracked().ok_or("Notes are reconnecting. Try again in a moment.")?;
                let detail = fetch_detail(&conn, &note.id).await?;
                let text = read_file_content(&note.id).await?;
                let found = text.get(context.start..context.end).filter(|f| f.to_lowercase() == context.matched.to_lowercase()).ok_or("The note changed. Refresh and try again.")?;
                let stem = target.rsplit('/').next().unwrap_or(&target);
                let link = if found == stem { format!("[[{target}]]") } else { format!("[[{target}|{found}]]") };
                let next = format!("{}{link}{}", &text[..context.start], &text[context.end..]);
                let entry = &detail.entry;
                let file = TextFile { name: &entry.name, parent_id: &entry.parent_id, content: &next, media_type: &entry.media_type, file_id: Some(&entry.id), expected_revision: Some(entry.revision) };
                write_text_file(&session.get_value(), file).await.map_err(|e| e.message)?;
                Ok(())
            }
            .await;
            if !alive.load(Ordering::Relaxed) {
                return;
            }
            busy.set(false);
            match result {
                Ok(()) => {
                    on_changed.try_run(());
                }
                Err(e) => error.set(e),
            }
        });
    };
    let link_mention = StoredValue::new(link_mention);

    let reference = move |note: RefNote, mention_target: Option<String>| {
        let open_id = note.id.clone();
        let folder = folder_of(&note.path).to_owned();
        let title = if note.title.is_empty() { "Untitled".to_owned() } else { note.title.clone() };
        let contexts = note.contexts.clone();
        view! {
            <article class="note-reference">
                <header>
                    <button type="button" class="note-reference-title" on:click=move |_| { on_open.try_run(open_id.clone()); }>{title}</button>
                    {(!folder.is_empty()).then(|| view! { <small>{folder}</small> })}
                </header>
                {contexts.into_iter().map(|context| {
                    let source = context.text.clone();
                    let action = mention_target.clone().map(|target| {
                        let (note, context) = (note.clone(), context.clone());
                        view! {
                            <button type="button" class="text-button" disabled=move || busy.get()
                                aria-label=format!("Link \u{201c}{}\u{201d} in {}", context.matched, note.title)
                                on:click=move |_| link_mention.with_value(|f| f(note.clone(), context.clone(), target.clone()))>"Link"</button>
                        }
                    });
                    view! {
                        <div class="note-reference-context" data-line=context.line>
                            <Markdown source=source links=links inline=true/>
                            {action}
                        </div>
                    }
                }).collect_view()}
            </article>
        }
    };

    view! {
        <section class="note-references" aria-label="References">
            {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
            <details class="note-references-group" open>
                <summary>
                    <h2>"Linked references"</h2>
                    <span class="note-references-count">{move || backlinks.with(|b| b.as_ref().map(|b| b.len().to_string()).unwrap_or_default())}</span>
                </summary>
                {move || match backlinks.get() {
                    None => view! { <p class="muted small" role="status">"Loading references\u{2026}"</p> }.into_any(),
                    Some(list) if list.is_empty() => view! { <p class="muted small">"No notes link here yet."</p> }.into_any(),
                    Some(list) => list.into_iter().map(|n| reference(n, None)).collect_view().into_any(),
                }}
            </details>
            <details class="note-references-group" on:toggle=move |e| {
                let open = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlDetailsElement>().ok()).is_some_and(|d| d.open());
                mentions_open.set(open);
            }>
                <summary>
                    <h2>"Unlinked references"</h2>
                    <span class="note-references-count">{move || mentions.with(|m| m.as_ref().map(|m| m.mentions.len().to_string()).unwrap_or_default())}</span>
                </summary>
                {move || match mentions.get() {
                    None => view! { <p class="muted small" role="status">"Looking for mentions\u{2026}"</p> }.into_any(),
                    Some(list) if list.mentions.is_empty() => view! { <p class="muted small">"No other notes mention this one by name."</p> }.into_any(),
                    Some(list) => {
                        let target = list.target.clone();
                        let more = list.truncated.then(|| view! { <p class="muted small">"Only the first 50 notes are shown."</p> });
                        view! { {list.mentions.into_iter().map(|n| reference(n, Some(target.clone()))).collect_view()}{more} }.into_any()
                    }
                }}
            </details>
        </section>
    }
}


fn attachment_name(file: &web_sys::File) -> String {
    let media = file.type_();
    let name: String = file.name().chars().map(|c| if matches!(c, '[' | ']' | '|' | '#' | '^' | '/' | '\\') || (c as u32) < 0x20 { '-' } else { c }).collect();
    let name = name.trim().to_owned();
    // Clipboard images arrive as `image.png`; name them as Obsidian does.
    if name.is_empty() || (media.starts_with("image/") && name.eq_ignore_ascii_case(&format!("image.{}", media.trim_start_matches("image/")))) {
        let now = js_sys::Date::new_0();
        let ext = media.split('/').nth(1).unwrap_or("bin").split('+').next().unwrap_or("bin");
        return format!(
            "Pasted image {:04}{:02}{:02}{:02}{:02}{:02}.{ext}",
            now.get_full_year(),
            now.get_month() + 1,
            now.get_date(),
            now.get_hours(),
            now.get_minutes(),
            now.get_seconds()
        );
    }
    name
}

/// Uploads pasted or dropped files to the Notes folder's `attachments` folder (made on first
/// use) and returns the embeds to insert, one per line.
pub async fn attach(live: Live, session: &BrowserSession, index: &NoteIndex, files: Vec<web_sys::File>) -> Result<String, String> {
    let conn = live.connection.get_untracked().ok_or("Notes are reconnecting. Try again in a moment.")?;
    if index.root_id.is_empty() {
        return Err("The Notes folder is still loading.".into());
    }
    let folder = match &index.attachments {
        Some(id) => id.clone(),
        None => {
            let id = crate::random_uuid();
            let (folder, parent) = (id.clone(), index.root_id.clone());
            reducer(|tx| conn.reducers.create_folder_then(folder, parent, "attachments".into(), replied(tx))).await.map_err(|_| "The attachments folder could not be created. Refresh and try again.".to_owned())?;
            id
        }
    };
    let mut taken: std::collections::HashSet<String> = index.files.iter().filter(|f| f.parent_id == folder).map(|f| f.name.to_lowercase()).collect();
    let mut embeds = Vec::new();
    for file in files {
        let wanted = attachment_name(&file);
        let (stem, ext) = wanted.rsplit_once('.').map_or((wanted.as_str(), String::new()), |(s, e)| (s, format!(".{e}")));
        let mut name = wanted.clone();
        let mut n = 1;
        while taken.contains(&name.to_lowercase()) {
            n += 1;
            name = format!("{stem} {n}{ext}");
        }
        let media = if file.type_().is_empty() { "application/octet-stream".to_owned() } else { file.type_() };
        upload_blob(session, &name, &folder, &media, &file).await.map_err(|e| format!("{name} could not be uploaded: {}", e.message))?;
        taken.insert(name.to_lowercase());
        embeds.push(format!("![[{name}]]"));
    }
    Ok(embeds.join("\n"))
}

/// Target note (and optional `#heading`/`#^block` fragment) for the hover preview card.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewTarget {
    pub id: String,
    pub target: String,
    pub fragment: Option<String>,
    pub title: String,
    /// Anchor link bounding rect: `(left, top, right, bottom)`.
    pub rect: (f64, f64, f64, f64),
}

/// Extracts a resolved note link's preview target from a hovered or focused element inside the
/// note preview (skipping unresolved links, references and embedded notes).
pub fn preview_target_from_element(element: &web_sys::Element, index: &NoteIndex) -> Option<PreviewTarget> {
    let link = element.closest("a.note-link:not(.is-unresolved)").ok().flatten()?;
    if link.closest(".note-references, .note-transclusion").ok().flatten().is_some() {
        return None;
    }
    let id = link
        .get_attribute("href")?
        .strip_prefix("/notes?note=")?
        .to_owned();
    if id.is_empty() {
        return None;
    }
    let raw_target = link.get_attribute("data-target").unwrap_or_default();
    let (parsed_target, fragment) = split_fragment(&raw_target);
    let (target, base_title) = match index.position(&id) {
        Some(pos) => {
            let wt = index.vault.wiki_target(pos);
            let note_title = &index.notes[pos].title;
            let title = if note_title.is_empty() { wt.clone() } else { note_title.clone() };
            (wt, title)
        }
        None => {
            let fallback = if parsed_target.is_empty() { "Note".to_owned() } else { parsed_target.clone() };
            (parsed_target, fallback)
        }
    };
    let title = match &fragment {
        Some(f) => format!("{base_title} \u{203a} {}", f.trim_start_matches('^')),
        None => base_title,
    };
    let r = link.get_bounding_client_rect();
    Some(PreviewTarget { id, target, fragment, title, rect: (r.left(), r.top(), r.right(), r.bottom()) })
}

/// Clamps the hover preview card position to the viewport, placing it below the link when it fits
/// or flipping above when near the bottom edge.
pub fn clamp_preview_position(rect: (f64, f64, f64, f64), card_w: f64, card_h: f64, view_w: f64, view_h: f64) -> (f64, f64) {
    let (left, top, _right, bottom) = rect;
    let margin = 8.0;
    let gap = 6.0;
    let max_left = (view_w - margin - card_w).max(margin);
    let x = left.clamp(margin, max_left);
    let below = bottom + gap;
    let y = if below + card_h <= view_h - margin || top - gap - card_h < margin {
        below.clamp(margin, (view_h - margin - card_h).max(margin))
    } else {
        (top - gap - card_h).max(margin)
    };
    (x.round(), y.round())
}

/// Floating top-layer hover/focus preview card for resolved note links in the note preview.
#[component]
pub fn NoteLinkPreview(
    target: RwSignal<Option<PreviewTarget>>,
    links: NoteLinks,
    on_enter: Callback<()>,
    on_leave: Callback<()>,
    on_close: Callback<()>,
) -> impl IntoView {
    let panel = NodeRef::<html::Div>::new();
    let card_links = NoteLinks {
        follow: links.follow.map(|f| {
            Callback::new(move |args| {
                on_close.try_run(());
                f.try_run(args);
            })
        }),
        tag: links.tag.map(|f| {
            Callback::new(move |t| {
                on_close.try_run(());
                f.try_run(t);
            })
        }),
        toggle: None,
        nested: true,
        ..links
    };

    Effect::new(move |_| {
        let current = target.get();
        let Some(element) = panel.get() else { return };
        let element: web_sys::HtmlElement = element.unchecked_into();
        let shown = element.matches(":popover-open").unwrap_or(false);
        let Some(item) = current else {
            if shown {
                call_method(&element, "hidePopover");
            }
            return;
        };
        if !shown {
            call_method(&element, "showPopover");
        }
        let place = {
            let element = element.clone();
            let rect = item.rect;
            move || {
                let view_w = window().inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
                let view_h = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
                let card_w = (element.offset_width() as f64).max(200.0);
                let card_h = (element.offset_height() as f64).max(60.0);
                let (x, y) = clamp_preview_position(rect, card_w, card_h, view_w, view_h);
                let style = element.style();
                let _ = style.set_property("--float-left", &format!("{x}px"));
                let _ = style.set_property("--float-top", &format!("{y}px"));
            }
        };
        place();
        let place = std::rc::Rc::new(place);
        let observer_cb = {
            let place = place.clone();
            Closure::<dyn FnMut()>::new(move || place())
        };
        let observer = web_sys::ResizeObserver::new(observer_cb.as_ref().unchecked_ref()).expect("ResizeObserver");
        observer.observe(&element);
        let resize = Listener::new(&window(), "resize", false, {
            let place = place.clone();
            move |_| place()
        });
        let kept = send_wrapper::SendWrapper::new((observer, observer_cb, resize, element));
        on_cleanup(move || {
            let (observer, _cb, _resize, element) = kept.take();
            observer.disconnect();
            if target.try_with_untracked(Option::is_some) != Some(true) && element.matches(":popover-open").unwrap_or(false) {
                call_method(&element, "hidePopover");
            }
        });
    });

    listen(&window(), "scroll", true, move |e| {
        let inside = panel
            .get_untracked()
            .zip(e.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()))
            .is_some_and(|(p, n)| p.contains(Some(&n)));
        if !inside {
            on_close.try_run(());
        }
    });

    listen(&window(), "keydown", true, move |e| {
        if let Some(ke) = e.dyn_ref::<web_sys::KeyboardEvent>()
            && ke.key() == "Escape"
            && target.try_with_untracked(Option::is_some) == Some(true)
        {
            ke.prevent_default();
            ke.stop_propagation();
            on_close.try_run(());
        }
    });

    view! {
        <div
            node_ref=panel
            popover="manual"
            class="note-preview-card"
            role="group"
            aria-label=move || target.with(|t| t.as_ref().map(|t| format!("Preview of {}", t.title)))
            on:mouseenter=move |_| { on_enter.try_run(()); }
            on:mouseleave=move |_| { on_leave.try_run(()); }
            on:focusin=move |_| { on_enter.try_run(()); }
            on:focusout=move |e: web_sys::FocusEvent| {
                let stays = panel
                    .get_untracked()
                    .zip(e.related_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()))
                    .is_some_and(|(p, n)| p.contains(Some(&n)));
                if !stays {
                    on_leave.try_run(());
                }
            }
        >
            {move || target.get().map(|item| note_embed(item.id, item.target, item.fragment, item.title, card_links))}
        </div>
    }
}

/// Six random lowercase alphanumeric characters for a `^block` id.
pub fn random_block_id() -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut bytes = [0u8; 6];
    let _ = getrandom::fill(&mut bytes);
    bytes.into_iter().map(|b| ALPHABET[(b as usize) % ALPHABET.len()] as char).collect()
}

/// Ensures the caret's current line carries a `^block` id. Returns `None` if the line is blank,
/// or `Some((id, updated_source))` where `updated_source` is `None` if the line already had an id.
pub fn ensure_block_id(source: &str, caret_byte: usize, new_id: impl FnOnce() -> String) -> Option<(String, Option<String>)> {
    let caret = caret_byte.min(source.len());
    let line_start = source[..caret].rfind('\n').map_or(0, |i| i + 1);
    let line_end = source[caret..].find('\n').map_or(source.len(), |i| caret + i);
    let content_end = if line_end > line_start && source.as_bytes()[line_end - 1] == b'\r' {
        line_end - 1
    } else {
        line_end
    };
    let line = &source[line_start..content_end];
    if line.trim().is_empty() {
        return None;
    }
    if let Some(existing) = dar_notes::block_id(line) {
        return Some((existing.to_owned(), None));
    }
    let id = new_id();
    let updated = format!("{}{} ^{id}{}", &source[..line_start], line.trim_end(), &source[content_end..]);
    Some((id, Some(updated)))
}

/// Writes `text` to `navigator.clipboard.writeText` via Reflect, returning `Err(())` when the
/// Clipboard API is unavailable or rejects.
pub async fn copy_to_clipboard(text: &str) -> Result<(), ()> {
    let nav = window().navigator();
    let clip = js_sys::Reflect::get(&nav, &JsValue::from_str("clipboard")).map_err(|_| ())?;
    if clip.is_undefined() || clip.is_null() {
        return Err(());
    }
    let write = js_sys::Reflect::get(&clip, &JsValue::from_str("writeText"))
        .map_err(|_| ())?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| ())?;
    let promise: js_sys::Promise = write
        .call1(&clip, &JsValue::from_str(text))
        .map_err(|_| ())?
        .dyn_into()
        .map_err(|_| ())?;
    wasm_bindgen_futures::JsFuture::from(promise).await.map_err(|_| ())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, path: &str, aliases: &[&str], tags: &[&str]) -> NotesFile {
        let title = path.rsplit('/').next().unwrap().trim_end_matches(".md").to_owned();
        NotesFile {
            id: id.into(),
            parent_id: String::new(),
            name: format!("{title}.md"),
            title,
            excerpt: String::new(),
            size: 0.0,
            revision: 1,
            modified_at: 0.0,
            favorite: false,
            path: path.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            aliases: aliases.iter().map(|t| t.to_string()).collect(),
            properties: Default::default(),
        }
    }

    fn index() -> NoteIndex {
        let library = NotesLibrary {
            root_id: "root".into(),
            path: "docs/notes".into(),
            folders: Vec::new(),
            notes: vec![note("a", "Project Apollo.md", &["Apollo"], &["work", "work/space"]), note("b", "Ideas.md", &[], &["work"]), note("c", "Old/Ideas.md", &[], &[])],
            truncated: false,
            legacy: 0,
        };
        NoteIndex::new(Some(&library))
    }

    #[test]
    fn triggers() {
        assert_eq!(trigger("See [[Proj", 10), Some(Trigger::Link { start: 4, query: "Proj".into() }));
        assert_eq!(trigger("See [[Proj]] x", 14), None);
        assert_eq!(trigger("a [[x|y", 7), None);
        assert_eq!(trigger("note #wo", 8), Some(Trigger::Tag { start: 5, query: "wo".into() }));
        assert_eq!(trigger("#", 1), Some(Trigger::Tag { start: 0, query: String::new() }));
        assert_eq!(trigger("a#wo", 4), None);
        assert_eq!(trigger("# Heading", 9), None);
    }

    #[test]
    fn suggests_notes_and_tags() {
        let ix = index();
        let found = suggestions(&ix, &Trigger::Link { start: 0, query: "apo".into() }, "b");
        assert_eq!(found[0].insert, "[[Project Apollo|Apollo]]");
        let found = suggestions(&ix, &Trigger::Link { start: 0, query: "ideas".into() }, "a");
        assert_eq!(found.iter().map(|s| s.insert.as_str()).collect::<Vec<_>>(), ["[[Ideas]]", "[[Old/Ideas]]"]);
        let tags = suggestions(&ix, &Trigger::Tag { start: 0, query: "wo".into() }, "a");
        assert_eq!(tags.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["#work", "#work/space"]);
        assert!(has_tag(&ix.notes[0], "Work"));
        assert!(!has_tag(&ix.notes[2], "work"));
    }

    #[test]
    fn accepts_completions() {
        let text = "See [[Pro]] now";
        let (next, caret) = accept(text, 9, &Trigger::Link { start: 4, query: "Pro".into() }, "[[Project Apollo]]");
        assert_eq!(next, "See [[Project Apollo]] now");
        assert_eq!(&next[..caret], "See [[Project Apollo]]");
        let (next, caret) = accept("go #wo", 6, &Trigger::Tag { start: 3, query: "wo".into() }, "#work ");
        assert_eq!((next.as_str(), caret), ("go #work ", 9));
        let (next, caret) = accept("x\n/co", 5, &Trigger::Slash { start: 2, query: "co".into() }, &format!("```\n{CARET}\n```"));
        assert_eq!(next, "x\n```\n\n```");
        assert_eq!(caret, 6);
        assert_eq!(trigger("a /tab", 6), Some(Trigger::Slash { start: 2, query: "tab".into() }));
        assert_eq!(trigger("https://x", 9), None);
        assert_eq!(fill_template("---\ntags: [t]\n---\n# {{title}}\n{{date:YYYY}} {{time}} {{other}}", "Plan", "2026-11-22", "09:30"), "# Plan\n2026-11-22 09:30 {{other}}");
    }

    #[test]
    fn block_link_ids_and_clamping() {
        assert_eq!(ensure_block_id("", 0, || "abc123".into()), None);
        assert_eq!(ensure_block_id("one\n   \ntwo", 5, || "abc123".into()), None);
        assert_eq!(
            ensure_block_id("one\ntwo  \nthree", 6, || "abc123".into()),
            Some(("abc123".into(), Some("one\ntwo ^abc123\nthree".into())))
        );
        assert_eq!(
            ensure_block_id("one\r\ntwo\r\nthree", 6, || "abc123".into()),
            Some(("abc123".into(), Some("one\r\ntwo ^abc123\r\nthree".into())))
        );
        assert_eq!(
            ensure_block_id("keep ^exist1\nnext", 4, || "unused".into()),
            Some(("exist1".into(), None))
        );
        let id = random_block_id();
        assert_eq!(id.len(), 6);
        assert!(id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));

        assert_eq!(clamp_preview_position((400.0, 200.0, 480.0, 220.0), 380.0, 240.0, 1440.0, 950.0), (400.0, 226.0));
        assert_eq!(clamp_preview_position((1200.0, 200.0, 1280.0, 220.0), 380.0, 240.0, 1440.0, 950.0), (1052.0, 226.0));
        assert_eq!(clamp_preview_position((400.0, 800.0, 480.0, 820.0), 380.0, 240.0, 1440.0, 950.0), (400.0, 554.0));
    }
}
