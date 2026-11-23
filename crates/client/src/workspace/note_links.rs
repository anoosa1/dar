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
use wasm_bindgen::JsCast;

use super::notes::{NotesFile, NotesLibrary, fetch_detail, has_md_ext, read_file_content};
use crate::api::{self, BrowserSession};
use crate::files::editor::{TextFile, write_text_file};
use crate::live::Live;
use crate::markdown::{Markdown, NoteLinks};
use crate::ui::{call_method, document, window};

/// The whole library, for resolving links (the list on screen may be a search result).
pub struct NoteIndex {
    pub vault: Vault,
    pub notes: Vec<NotesFile>,
    /// Tags with the number of notes carrying them, by name.
    pub tags: Vec<(String, usize)>,
}

impl NoteIndex {
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
        Self { vault, notes, tags }
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
                found.push((score, note.modified_at, Suggestion { label, detail: detail.trim_end_matches([' ', '\u{b7}']).to_owned(), insert }));
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
                .map(|(_, n, t)| Suggestion { label: format!("#{t}"), detail: format!("{n} {}", if n == 1 { "note" } else { "notes" }), insert: format!("#{t} ") })
                .collect()
        }
    }
}

/// The text with a suggestion accepted at `caret` (bytes), and the caret after it.
pub fn accept(text: &str, caret: usize, trigger: &Trigger, insert: &str) -> (String, usize) {
    let (start, mut end) = match trigger {
        Trigger::Link { start, .. } | Trigger::Tag { start, .. } => (*start, caret),
    };
    if matches!(trigger, Trigger::Link { .. }) && text[end..].starts_with("]]") {
        end += 2;
    }
    if matches!(trigger, Trigger::Tag { .. }) && text[end..].starts_with(' ') {
        end += 1;
    }
    let next = format!("{}{insert}{}", &text[..start], &text[end..]);
    (next, start + insert.len())
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
pub async fn retarget_links(live: Live, session: &BrowserSession, before: &NoteIndex, moved: &str, new_path: &str, sources: Vec<String>) -> Result<usize, String> {
    let Some(moved_index) = before.position(moved) else { return Ok(0) };
    let after = Vault::new(before.vault.paths().iter().enumerate().map(|(i, p)| if i == moved_index { new_path.to_owned() } else { p.clone() }));
    let mut ids = sources;
    if !ids.iter().any(|i| i == moved) {
        ids.push(moved.to_owned());
    }
    let mut changed = 0;
    let mut failed = 0;
    for id in ids {
        let Some(old_source) = before.path_of(&id).map(str::to_owned) else { continue };
        let new_source = if id == moved { new_path.to_owned() } else { old_source.clone() };
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
            if hit == moved_index || (id == moved && !link.wiki && folder_of(&old_source) != folder_of(&new_source)) {
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
    }
}
