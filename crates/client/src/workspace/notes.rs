//! Notes (`notes.tsx`): Markdown files in a Files folder (`docs/notes` by default), with
//! folder navigation, search, favorites, heading-based naming on close, legacy notebook import,
//! and the shared Markdown text editor.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use super::note_graph::NoteGraph;
use super::note_links::{self, LinkSuggestions, NoteIndex, NoteReferences, Suggestion, Trigger, has_tag};
use super::{ReactiveSelect, WorkspaceShell, search_params};
use crate::api::{self, BrowserSession};
use crate::dates::edited;
use crate::files::FileEntry;
use crate::files::editor::{TextFile, editable_text, write_text_file};
use crate::files::types::{FileTile, file_type_label};
use crate::icons::Icon;
use crate::live::{Connection, Live, answered, procedure, reducer, replied};
use crate::markdown::{Markdown, MarkdownToolbar, NoteLinks, byte_index, format_markdown, utf16_len};
use crate::module_bindings::{create_folder, favorite_entry, file_details, move_entry, rename_entry, trash_entry};
use crate::pim::{Collection, use_pim};
use crate::shared::{context, save_preferences};
use crate::ui::{ActionMenu, Dialog, EditorPane, MenuAction, listen, unique_id, window};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesFolder {
    pub id: String,
    pub parent_id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub modified_at: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesFile {
    pub id: String,
    pub parent_id: String,
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub excerpt: String,
    #[serde(default)]
    pub size: f64,
    pub revision: u32,
    #[serde(default)]
    pub modified_at: f64,
    #[serde(default)]
    pub favorite: bool,
    pub path: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub properties: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesLibrary {
    pub root_id: String,
    pub path: String,
    #[serde(default)]
    pub folders: Vec<NotesFolder>,
    #[serde(default)]
    pub notes: Vec<NotesFile>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub legacy: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct NoteDetail {
    pub entry: FileEntry,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq)]
enum NotesPaneKind {
    Folder { parent_id: String },
    RenameFolder(NotesFolder),
    RenameNote(NotesFile),
    Move(NotesFile),
}

#[derive(Clone)]
enum ConfirmTarget {
    Note(NotesFile),
    Folder(NotesFolder),
}

#[derive(Clone)]
struct ConfirmState {
    title: String,
    text: String,
    action: String,
    target: ConfirmTarget,
}

fn error_message(text: &str) -> String {
    let lower = text.to_lowercase();
    if lower.contains("conflict") {
        "That name is already used here, or the item changed. Refresh and try again.".into()
    } else if lower.contains("forbidden") || lower.contains("unauthenticated") {
        "Your access changed. Refresh the page or sign in again.".into()
    } else if text.is_empty() {
        "The request could not be completed.".into()
    } else {
        text.to_owned()
    }
}

pub(super) fn strip_md_ext(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.ends_with(".markdown") {
        name[..name.len() - 9].to_owned()
    } else if lower.ends_with(".md") {
        name[..name.len() - 3].to_owned()
    } else {
        name.to_owned()
    }
}

pub(super) fn has_md_ext(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// File names keep the title readable and drop characters Files does not allow.
fn file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if (c as u32) < 0x20 || c == '/' || c == '\\' { ' ' } else { c })
        .collect();
    let collapsed: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = collapsed.chars().take(200).collect();
    let stem = trimmed.trim();
    format!("{}.md", if stem.is_empty() { "Untitled" } else { stem })
}

fn unique_name(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(&name.to_lowercase()) {
        return name.to_owned();
    }
    let lower = name.to_lowercase();
    let base = if lower.ends_with(".md") { &name[..name.len() - 3] } else { name };
    for n in 2..1000 {
        let next = format!("{base} {n}.md");
        if !taken.contains(&next.to_lowercase()) {
            return next;
        }
    }
    format!("{base} {}.md", js_sys::Date::now() as u64)
}

fn is_untitled_md(name: &str) -> bool {
    let lower = name.to_lowercase();
    let Some(stem) = lower.strip_suffix(".md") else { return false };
    let Some(rest) = stem.strip_prefix("untitled") else { return false };
    if rest.is_empty() {
        return true;
    }
    rest.strip_prefix(' ').is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

fn extract_heading(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix('#')
            && rest.starts_with(char::is_whitespace)
        {
            let trimmed = rest.trim().trim_end_matches('#').trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

pub(super) async fn fetch_detail(conn: &Connection, id: &str) -> Result<NoteDetail, String> {
    let id = id.to_owned();
    procedure(|tx| conn.procedures.file_details_then(id, answered(tx))).await
}

fn session_storage() -> Option<web_sys::Storage> {
    window().session_storage().ok().flatten()
}

pub(super) async fn read_file_content(id: &str) -> Result<String, String> {
    let response = api::fetch(&format!("/api/v1/files/{id}/content"), Default::default()).await.map_err(|e| e.message)?;
    if !response.ok() {
        return Err("The file could not be loaded.".into());
    }
    let text = wasm_bindgen_futures::JsFuture::from(response.text().map_err(|_| "The file could not be loaded.")?)
        .await
        .map_err(|_| "The file could not be loaded.")?;
    Ok(text.as_string().unwrap_or_default())
}

#[component]
pub fn Notes(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let pim = use_pim(on_signed_out);
    let live = expect_context::<Live>();
    let params = web_sys::UrlSearchParams::new_with_str(&window().location().search().unwrap_or_default()).expect("params");
    let initial_scope = params.get("folder").unwrap_or_else(|| "all".into());
    let initial_note: RwSignal<Option<String>> = RwSignal::new(params.get("note").or_else(|| params.get("file")));

    let library: RwSignal<Option<NotesLibrary>> = RwSignal::new(None);
    // The whole library (the list may show a search result), for links, tags and suggestions.
    let all_notes: RwSignal<Option<NotesLibrary>> = RwSignal::new(None);
    let attachments: RwSignal<Option<note_links::NoteFiles>> = RwSignal::new(None);
    let index = RwSignal::new(Arc::new(NoteIndex::new(None)));
    Effect::new(move |_| all_notes.with(|l| attachments.with(|a| index.set(Arc::new(NoteIndex::new(l.as_ref()).with_files(a.as_ref()))))));
    // A heading or block to show once a note opens: (note ID, fragment).
    let anchor: RwSignal<Option<(String, String)>> = RwSignal::new(None);
    // Bumped when other notes were rewritten (link updates), so a clean editor reloads.
    let refresh = RwSignal::new(0u32);
    let preview_choice: RwSignal<Option<bool>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let query = RwSignal::new(String::new());
    let scope = RwSignal::new(initial_scope);
    let open: RwSignal<Option<FileEntry>> = RwSignal::new(None);
    let pane: RwSignal<Option<NotesPaneKind>> = RwSignal::new(None);
    let confirm: RwSignal<Option<ConfirmState>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let reload = RwSignal::new(0u32);
    let last_saved: RwSignal<Option<(String, String)>> = RwSignal::new(None);
    let load_seq = RwSignal::new(0u32);
    let session_sv = StoredValue::new(session.clone());

    let shared_prefs = context().preferences;
    let sort = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("notesSort").and_then(Value::as_str))
                .unwrap_or("modified")
                .to_owned()
        })
    });
    let notes_imported = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("notesImported").and_then(Value::as_bool))
                .unwrap_or(false)
        })
    });
    let start_in_preview = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("notesEditorMode").and_then(Value::as_str))
                == Some("preview")
        })
    });

    // Notes change in Files too (WebDAV, other apps), so the list refreshes on focus and epoch changes.
    Effect::new(move |_| {
        let q = query.get();
        let _r = reload.get();
        let _e = pim.epoch.get();
        let seq = load_seq.get_untracked() + 1;
        load_seq.set(seq);
        spawn_local(async move {
            if !q.is_empty() {
                crate::sleep(200).await;
                if load_seq.try_get_untracked() != Some(seq) {
                    return;
                }
            }
            let filtered = !q.is_empty();
            let qs = search_params(&[("q", q)]);
            let whole = if filtered { api::get::<NotesLibrary>("/api/v1/notes/library").await.ok() } else { None };
            if let Ok(files) = api::get::<note_links::NoteFiles>("/api/v1/notes/attachments").await
                && load_seq.try_get_untracked() == Some(seq)
            {
                attachments.set(Some(files));
            }
            match api::get::<NotesLibrary>(&format!("/api/v1/notes/library?{qs}")).await {
                Ok(value) if load_seq.try_get_untracked() == Some(seq) => {
                    all_notes.set(if filtered { whole.or_else(|| all_notes.get_untracked()) } else { Some(value.clone()) });
                    library.set(Some(value));
                    error.set(String::new());
                }
                Err(cause) if load_seq.try_get_untracked() == Some(seq) => {
                    error.set(error_message(&cause.message));
                }
                _ => {}
            }
        });
    });

    listen(&window(), "focus", false, move |_| reload.update(|n| *n += 1));

    // Keep URL query params (?note=, ?folder=) synchronized.
    Effect::new(move |_| {
        let open_id = open.with(|o| o.as_ref().map(|e| e.id.clone()));
        let current_scope = scope.get();
        let Ok(url) = web_sys::Url::new(&window().location().href().unwrap_or_default()) else { return };
        let sp = url.search_params();
        sp.delete("file");
        match open_id {
            Some(id) => sp.set("note", &id),
            None => sp.delete("note"),
        }
        if current_scope == "all" {
            sp.delete("folder");
        } else {
            sp.set("folder", &current_scope);
        }
        let search = sp.to_string().as_string().unwrap_or_default();
        let next = if search.is_empty() { url.pathname() } else { format!("{}?{search}", url.pathname()) };
        if let Ok(history) = window().history() {
            let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&next));
        }
    });

    // Open initial deep-linked note once the live connection is ready.
    Effect::new(move |_| {
        let Some(conn) = live.connection.get() else { return };
        let Some(id) = initial_note.get_untracked() else { return };
        initial_note.set(None);
        spawn_local(async move {
            match fetch_detail(&conn, &id).await {
                Ok(detail) => open.set(Some(detail.entry)),
                Err(cause) => error.set(error_message(&cause)),
            }
        });
    });

    let folder_name = move |id: &str| -> String {
        library.with(|lib| {
            let Some(lib) = lib else { return "Notes".into() };
            if id == lib.root_id {
                return "Notes".into();
            }
            lib.folders.iter().find(|f| f.id == id).map(|f| f.name.clone()).unwrap_or_else(|| "Notes".into())
        })
    };

    let siblings = move |parent_id: &str| -> HashSet<String> {
        library.with_untracked(|lib| {
            lib.as_ref()
                .map(|l| l.notes.iter().filter(|n| n.parent_id == parent_id).map(|n| n.name.to_lowercase()).collect())
                .unwrap_or_default()
        })
    };

    let current_target = move || -> String {
        let s = scope.get_untracked();
        library.with_untracked(|lib| {
            let Some(lib) = lib else { return String::new() };
            if s != "all" && s != "favorites"
                && let Some(folder) = lib.folders.iter().find(|f| f.id == s)
            {
                return folder.id.clone();
            }
            lib.root_id.clone()
        })
    };

    let new_note = move || {
        if library.with_untracked(Option::is_none) {
            return;
        }
        let target = current_target();
        let name = unique_name("Untitled.md", &siblings(&target));
        let sess = session_sv.get_value();
        spawn_local(async move {
            busy.set(true);
            error.set(String::new());
            let result: Result<(), String> = async {
                let conn = live.connection.get_untracked().ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
                let id = write_text_file(
                    &sess,
                    TextFile { name: &name, parent_id: &target, content: "", media_type: "text/markdown", file_id: None, expected_revision: None },
                )
                .await
                .map_err(|e| e.message)?;
                let detail = fetch_detail(&conn, &id).await?;
                open.set(Some(detail.entry));
                Ok(())
            }
            .await;
            if let Err(e) = result {
                error.set(error_message(&e));
            } else {
                reload.update(|n| *n += 1);
            }
            busy.set(false);
        });
    };

    let open_note = move |note_id: String| {
        spawn_local(async move {
            let Some(conn) = live.connection.get_untracked() else {
                error.set("Notes are reconnecting. Try again in a moment.".into());
                return;
            };
            match fetch_detail(&conn, &note_id).await {
                Ok(detail) => open.set(Some(detail.entry)),
                Err(cause) => error.set(error_message(&cause)),
            }
        });
    };

    // A link to a missing note creates it (in the folder the link names, else the linking note's
    // folder for a Markdown link, else the Notes folder) and opens it.
    let create_linked = move |source_id: String, target: String, wiki: bool| {
        let Some(root) = library.with_untracked(|l| l.as_ref().map(|l| l.root_id.clone())) else { return };
        let stem = strip_md_ext(target.rsplit('/').next().unwrap_or(&target).trim());
        let folder_path = if wiki {
            target.rfind('/').map(|i| target[..i].trim_matches('/').to_owned()).unwrap_or_default()
        } else {
            index.with_untracked(|ix| ix.path_of(&source_id).and_then(|p| p.rfind('/').map(|i| p[..i].to_owned()))).unwrap_or_default()
        };
        let parent = all_notes.with_untracked(|l| {
            l.as_ref().and_then(|l| l.folders.iter().find(|f| !folder_path.is_empty() && f.path.to_lowercase() == folder_path.to_lowercase()).map(|f| f.id.clone()))
        });
        let parent = parent.unwrap_or(root);
        let name = unique_name(&file_name(&stem), &siblings(&parent));
        let sess = session_sv.get_value();
        spawn_local(async move {
            busy.set(true);
            let result: Result<(), String> = async {
                let conn = live.connection.get_untracked().ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
                let file = TextFile { name: &name, parent_id: &parent, content: "", media_type: "text/markdown", file_id: None, expected_revision: None };
                let id = write_text_file(&sess, file).await.map_err(|e| e.message)?;
                open.set(Some(fetch_detail(&conn, &id).await?.entry));
                Ok(())
            }
            .await;
            match result {
                Ok(()) => reload.update(|n| *n += 1),
                Err(e) => error.set(error_message(&e)),
            }
            busy.set(false);
        });
    };

    let follow_link = move |source_id: String, target: String, wiki: bool, fragment: Option<String>| {
        let hit = index.with_untracked(|ix| ix.resolve(&source_id, &target, wiki).map(|n| n.id.clone()));
        match hit {
            Some(id) => {
                anchor.set(fragment.map(|f| (id.clone(), f)));
                if id != source_id {
                    open_note(id);
                }
            }
            None if wiki && !dar_notes::is_note_target(&target) => notice.set(format!("\u{201c}{target}\u{201d} is not a note in this folder.")),
            None => create_linked(source_id, target, wiki),
        }
    };

    // After a rename or move: links elsewhere follow the note (Obsidian's automatic link updates).
    let relink = move |moves: Vec<(String, String)>, sources: Vec<String>, before: Arc<NoteIndex>| async move {
        let sess = session_sv.get_value();
        let outcome = note_links::retarget_links(live, &sess, &before, &moves, sources).await;
        refresh.update(|n| *n += 1);
        match outcome {
            Ok(0) => String::new(),
            Ok(n) => format!(" Updated links in {n} {}.", if n == 1 { "note" } else { "notes" }),
            Err(e) => {
                error.set(e);
                String::new()
            }
        }
    };
    let folder_path = move |folder_id: &str| -> String {
        all_notes.with_untracked(|l| l.as_ref().filter(|l| l.root_id != folder_id).and_then(|l| l.folders.iter().find(|f| f.id == folder_id).map(|f| f.path.clone()))).unwrap_or_default()
    };
    let joined = |folder: String, name: &str| if folder.is_empty() { name.to_owned() } else { format!("{folder}/{name}") };

    // Today's daily note (`Journal/YYYY-MM-DD.md`), made from `Templates/Daily` when there is one.
    let open_today = move || {
        let (date, time) = note_links::today();
        let ix = index.get_untracked();
        if let Some(i) = ix.vault.find(&format!("Journal/{date}.md")) {
            open_note(ix.notes[i].id.clone());
            return;
        }
        if ix.root_id.is_empty() {
            return;
        }
        let sess = session_sv.get_value();
        let journal = all_notes.with_untracked(|l| l.as_ref().and_then(|l| l.folders.iter().find(|f| f.path.eq_ignore_ascii_case("journal")).map(|f| f.id.clone())));
        spawn_local(async move {
            busy.set(true);
            let result: Result<(), String> = async {
                let conn = live.connection.get_untracked().ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
                let folder = match journal {
                    Some(id) => id,
                    None => {
                        let id = crate::random_uuid();
                        let (folder, parent) = (id.clone(), ix.root_id.clone());
                        reducer(|tx| conn.reducers.create_folder_then(folder, parent, "Journal".into(), replied(tx))).await?;
                        id
                    }
                };
                let template = ix.notes.iter().find(|n| note_links::is_template(n) && matches!(strip_md_ext(&n.name).to_lowercase().as_str(), "daily" | "daily note" | "journal"));
                let content = match template {
                    Some(t) => note_links::fill_template(&read_file_content(&t.id).await?, &date, &date, &time),
                    None => String::new(),
                };
                let name = format!("{date}.md");
                let file = TextFile { name: &name, parent_id: &folder, content: &content, media_type: "text/markdown", file_id: None, expected_revision: None };
                let id = write_text_file(&sess, file).await.map_err(|e| e.message)?;
                open.set(Some(fetch_detail(&conn, &id).await?.entry));
                Ok(())
            }
            .await;
            match result {
                Ok(()) => reload.update(|n| *n += 1),
                Err(e) => error.set(error_message(&e)),
            }
            busy.set(false);
        });
    };

    let toggle_favorite = move |note_id: String| {
        spawn_local(async move {
            busy.set(true);
            error.set(String::new());
            let result: Result<(), String> = async {
                let conn = live.connection.get_untracked().ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
                let entry = fetch_detail(&conn, &note_id).await?.entry;
                reducer(|tx| conn.reducers.favorite_entry_then(entry.id, !entry.favorite, entry.revision, replied(tx))).await
            }
            .await;
            if let Err(e) = result {
                error.set(error_message(&e));
            } else {
                reload.update(|n| *n += 1);
            }
            busy.set(false);
        });
    };

    let request_remove_note = move |note: NotesFile| {
        confirm.set(Some(ConfirmState {
            title: "Move note to deleted files?".into(),
            action: "Move to deleted files".into(),
            text: format!("\u{201c}{}\u{201d} can be restored from Files \u{203a} Deleted files.", note.title),
            target: ConfirmTarget::Note(note),
        }));
    };

    let request_remove_folder = move |folder: NotesFolder| {
        let prefix = format!("{}/", folder.path);
        let count = library.with_untracked(|lib| {
            lib.as_ref().map(|l| l.notes.iter().filter(|n| n.path.starts_with(&prefix)).count()).unwrap_or(0)
        });
        confirm.set(Some(ConfirmState {
            title: "Move folder to deleted files?".into(),
            action: "Move to deleted files".into(),
            text: format!(
                "\u{201c}{}\u{201d} and its {count} {} can be restored from Files \u{203a} Deleted files.",
                folder.name,
                if count == 1 { "note" } else { "notes" }
            ),
            target: ConfirmTarget::Folder(folder),
        }));
    };

    let build_note_actions = move |note: NotesFile| -> Vec<MenuAction> {
        let is_busy = busy.get_untracked();
        let has_folders = library.with_untracked(|lib| lib.as_ref().is_some_and(|l| !l.folders.is_empty()));
        let n_fav = note.id.clone();
        let n_ren = note.clone();
        let n_mov = note.clone();
        let n_del = note.clone();
        vec![
            MenuAction::new(if note.favorite { "Remove from favorites" } else { "Add to favorites" }, "star")
                .disabled(is_busy)
                .run(move || toggle_favorite(n_fav.clone())),
            MenuAction::new("Rename", "pencil")
                .disabled(is_busy)
                .run(move || pane.set(Some(NotesPaneKind::RenameNote(n_ren.clone())))),
            MenuAction::new("Move to\u{2026}", "move")
                .disabled(is_busy || !has_folders)
                .run(move || pane.set(Some(NotesPaneKind::Move(n_mov.clone())))),
            MenuAction::new("Show in Files", "folder")
                .href(format!("/?folder={}&file={}", note.parent_id, note.id)),
            MenuAction::new("Move to deleted files", "trash")
                .danger()
                .separator()
                .disabled(is_busy)
                .run(move || request_remove_note(n_del.clone())),
        ]
    };

    let sorted_folders = Memo::new(move |_| {
        let mut folders = library.with(|lib| lib.as_ref().map(|l| l.folders.clone()).unwrap_or_default());
        folders.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
        folders
    });

    let rows = Memo::new(move |_| {
        let s = scope.get();
        let current_sort = sort.get();
        library.with(|lib| {
            let Some(lib) = lib else { return Vec::new() };
            let tag = s.strip_prefix("tag:");
            let current_folder = if s != "all" && s != "favorites" && tag.is_none() {
                lib.folders.iter().find(|f| f.id == s)
            } else {
                None
            };
            let prefix = current_folder.map(|f| format!("{}/", f.path));
            let mut list: Vec<NotesFile> = lib
                .notes
                .iter()
                .filter(|note| {
                    if s == "all" {
                        true
                    } else if s == "favorites" {
                        note.favorite
                    } else if let Some(tag) = tag {
                        has_tag(note, tag)
                    } else if let Some(ref p) = prefix {
                        note.path.starts_with(p)
                    } else {
                        true
                    }
                })
                .cloned()
                .collect();
            if current_sort == "title" {
                list.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then_with(|| a.title.cmp(&b.title)));
            } else {
                list.sort_by(|a, b| b.modified_at.total_cmp(&a.modified_at));
            }
            list
        })
    });

    let heading_title = Memo::new(move |_| {
        let s = scope.get();
        if s == "favorites" {
            return "Favorites".to_owned();
        }
        if s == "graph" {
            return "Graph".to_owned();
        }
        if let Some(tag) = s.strip_prefix("tag:") {
            return format!("#{tag}");
        }
        if s != "all"
            && let Some(name) = library.with(|lib| lib.as_ref().and_then(|l| l.folders.iter().find(|f| f.id == s).map(|f| f.name.clone())))
        {
            return name;
        }
        "All notes".to_owned()
    });

    let current_folder_path = Memo::new(move |_| {
        let s = scope.get();
        if s == "all" || s == "favorites" || s == "graph" {
            return None;
        }
        library.with(|lib| lib.as_ref().and_then(|l| l.folders.iter().find(|f| f.id == s).map(|f| f.path.clone())))
    });

    let sidebar: ChildrenFn = Arc::new(move || {
        view! {
            <button type="button" class="new-button ui-new" disabled=move || busy.get() || library.with(Option::is_none) on:click=move |_| new_note()>
                <Icon name="plus"/>"New note"
            </button>
            <nav aria-label="Notes views">
                {[("all", "All notes", "list"), ("favorites", "Favorites", "star")].into_iter().map(|(value, label, icon)| {
                    view! {
                        <button class=move || format!("nav-item {}", if scope.get() == value { "active" } else { "" }) aria-current=move || (scope.get() == value).then_some("page") on:click=move |_| scope.set(value.to_owned())>
                            <Icon name=icon/>{label}
                        </button>
                    }
                }).collect_view()}
                <button class="nav-item" disabled=move || busy.get() || library.with(Option::is_none) on:click=move |_| open_today()>
                    <Icon name="calendar"/>"Today"
                </button>
                <button class=move || format!("nav-item {}", if scope.get() == "graph" { "active" } else { "" }) aria-current=move || (scope.get() == "graph").then_some("page") on:click=move |_| scope.set("graph".to_owned())>
                    <Icon name="share"/>"Graph"
                </button>
            </nav>
            <div class="sidebar-heading">
                "Folders"
                <button class="text-button" aria-label="New folder" disabled=move || library.with(Option::is_none) on:click=move |_| pane.set(Some(NotesPaneKind::Folder { parent_id: current_target() }))>"\u{ff0b}"</button>
            </div>
            <div class="notes-folders" role="list" aria-label="Folders">
                {move || sorted_folders.get().into_iter().map(|folder| {
                    let depth = (folder.path.split('/').count().saturating_sub(1)).min(5);
                    let id_click = folder.id.clone();
                    let id_active = folder.id.clone();
                    let id_current = folder.id.clone();
                    let id_inside = folder.id.clone();
                    let id_href = folder.id.clone();
                    let folder_rename = folder.clone();
                    let folder_del = folder.clone();
                    let folder_label = format!("Actions for folder {}", folder.name);
                    view! {
                        <div role="listitem" class=format!("notes-folder-row depth-{depth}")>
                            <button class=move || format!("nav-item {}", if scope.get() == id_active { "active" } else { "" }) aria-current=move || (scope.get() == id_current).then_some("page") on:click=move |_| scope.set(id_click.clone())>
                                <Icon name="folder"/><span>{folder.name.clone()}</span>
                            </button>
                            <ActionMenu label=folder_label class="contacts-book-actions" actions=move |_| vec![
                                MenuAction::new("New folder inside", "plus").run({ let id = id_inside.clone(); move || pane.set(Some(NotesPaneKind::Folder { parent_id: id.clone() })) }),
                                MenuAction::new("Rename folder", "pencil").run({ let f = folder_rename.clone(); move || pane.set(Some(NotesPaneKind::RenameFolder(f.clone()))) }),
                                MenuAction::new("Show in Files", "external").href(format!("/?folder={id_href}")),
                                MenuAction::new("Move to deleted files", "trash").danger().separator().run({ let f = folder_del.clone(); move || request_remove_folder(f.clone()) }),
                            ]><Icon name="more"/></ActionMenu>
                        </div>
                    }
                }).collect_view()}
                {move || library.with(|lib| lib.as_ref().filter(|l| l.folders.is_empty()).map(|l| view! {
                    <p class="muted small notes-folders-empty">"Folders in Files \u{203a} "{l.path.clone()}" appear here."</p>
                }))}
            </div>
            {move || {
                let tags = index.with(|ix| ix.tags.clone());
                (!tags.is_empty()).then(|| view! {
                    <div class="sidebar-heading">"Tags"</div>
                    <div class="notes-tags" role="list" aria-label="Tags">
                        {tags.into_iter().map(|(tag, count)| {
                            let value = format!("tag:{tag}");
                            let (v_class, v_current, v_click) = (value.clone(), value.clone(), value);
                            let name = tag.clone();
                            view! {
                                <div role="listitem" class="notes-tag-row">
                                    <button class=move || format!("nav-item {}", if scope.get() == v_class { "active" } else { "" }) aria-current=move || (scope.get() == v_current).then_some("page")
                                        aria-label=format!("#{tag}, {count} {}", if count == 1 { "note" } else { "notes" }) on:click=move |_| scope.set(v_click.clone())>
                                        <Icon name="tag"/><span>{name}</span><small class="nav-count">{count}</small>
                                    </button>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                })
            }}
        }
        .into_any()
    });

    let details: ChildrenFn = Arc::new(move || {
        view! {
            <For each=move || open.get().into_iter() key=|e| e.id.clone() let:initial>
                {
                    let init_id = initial.id.clone();
                    let source_id = StoredValue::new(initial.id.clone());
                    let entry_sig = Signal::derive(move || open.get().filter(|e| e.id == init_id).unwrap_or_else(|| initial.clone()));
                    let ctx_sig = Signal::derive(move || Some(folder_name(&entry_sig.with(|e| e.parent_id.clone()))));
                    let on_close = Callback::new(move |_| {
                        let closed = open.get_untracked();
                        open.set(None);
                        let saved = last_saved.get_untracked();
                        last_saved.set(None);
                        spawn_local(async move {
                            if let (Some(entry), Some((saved_id, saved_content)), Some(conn)) = (closed, saved, live.connection.get_untracked())
                                && saved_id == entry.id
                                && is_untitled_md(&entry.name)
                                && let Some(heading) = extract_heading(&saved_content)
                                && let Ok(detail) = fetch_detail(&conn, &entry.id).await
                            {
                                let current = detail.entry;
                                let next_name = unique_name(&file_name(&heading), &siblings(&current.parent_id));
                                if next_name != current.name {
                                    let before = index.get_untracked();
                                    let sources = note_links::backlink_ids(&current.id).await;
                                    let new_path = joined(folder_path(&current.parent_id), &next_name);
                                    let id = current.id.clone();
                                    if reducer(|tx| conn.reducers.rename_entry_then(current.id, next_name, current.revision, replied(tx))).await.is_ok() {
                                        let updated = relink(vec![(id, new_path)], sources, before).await;
                                        if !updated.is_empty() {
                                            notice.set(updated.trim_start().to_owned());
                                        }
                                    }
                                }
                            }
                            reload.update(|n| *n += 1);
                        });
                    });
                    let on_saved = Callback::new(move |(_rev, content): (u32, String)| {
                        if let Some(id) = open.with_untracked(|o| o.as_ref().map(|e| e.id.clone())) {
                            last_saved.set(Some((id, content)));
                        }
                        reload.update(|n| *n += 1);
                    });
                    let editor_actions = Callback::new(move |()| {
                        let current_id = entry_sig.with_untracked(|e| e.id.clone());
                        let row = library.with_untracked(|lib| lib.as_ref().and_then(|l| l.notes.iter().find(|n| n.id == current_id).cloned()));
                        row.map(build_note_actions).unwrap_or_default()
                    });
                    let links = NoteLinks {
                        resolve: Some(Callback::new(move |(target, wiki): (String, bool)| {
                            index.with(|ix| ix.resolve(&source_id.get_value(), &target, wiki).map(|n| n.id.clone()))
                        })),
                        follow: Some(Callback::new(move |(target, wiki, fragment): (String, bool, Option<String>)| follow_link(source_id.get_value(), target, wiki, fragment))),
                        tag: Some(Callback::new(move |tag: String| scope.set(format!("tag:{tag}")))),
                        toggle: None,
                        embed: Some(Callback::new(move |(target, wiki): (String, bool)| index.with(|ix| ix.embed(&source_id.get_value(), &target, wiki)))),
                        nested: false,
                        query: Some(Callback::new(move |source: String| index.with(|ix| super::note_query::run(ix, &source_id.get_value(), &source)))),
                    };
                    let version = RwSignal::new(0u32);
                    let on_saved = Callback::new(move |saved: (u32, String)| {
                        version.update(|n| *n += 1);
                        on_saved.run(saved);
                    });
                    let note_id = Signal::derive(move || source_id.get_value());
                    let starts_in_preview = preview_choice.get_untracked().unwrap_or_else(|| start_in_preview.get_untracked());
                    view! {
                        <NoteTextEditor entry=entry_sig session=session_sv.get_value() context=ctx_sig start_in_preview=starts_in_preview on_close=on_close on_saved=on_saved actions=editor_actions
                            links=links index=index anchor=anchor refresh=refresh on_mode=Callback::new(move |p: bool| preview_choice.set(Some(p)))>
                            <NoteReferences note_id=note_id version=Signal::derive(move || version.get() + refresh.get()) links=links session=session_sv.get_value()
                                on_open=Callback::new(move |id: String| open_note(id))
                                on_changed=Callback::new(move |_| { version.update(|n| *n += 1); reload.update(|n| *n += 1); })/>
                        </NoteTextEditor>
                    }
                }
            </For>
        }
        .into_any()
    });

    let has_open = Signal::from(Memo::new(move |_| open.with(Option::is_some)));
    let graph_epoch = pim.epoch;

    view! {
        <WorkspaceShell kind="notes" session=session on_signed_out=on_signed_out sidebar=sidebar has_details=has_open note_editor=has_open details=details pim=pim>
            <div class=move || if scope.get() == "graph" { "workspace-list notes-list is-graph" } else { "workspace-list notes-list" }>
                <div class="workspace-heading">
                    <h1>{move || heading_title.get()}</h1>
                    <Show when=move || scope.get() != "graph">
                        <ActionMenu label="Sort notes" class="workspace-sort" actions=move |_| {
                            let current = sort.get_untracked();
                            [("modified", "Recently edited"), ("title", "Title")].into_iter().map(|(value, label)| {
                                MenuAction::plain(label).current(current == value).run(move || {
                                    let rev = shared_prefs.value.with_untracked(|p| p.as_ref().map(|p| p.revision).unwrap_or(0));
                                    let sess = session_sv.get_value();
                                    spawn_local(async move {
                                        if let Err(cause) = save_preferences(json!({ "revision": rev, "notesSort": value }), rev, &sess).await {
                                            error.set(error_message(&cause.message));
                                        }
                                    });
                                })
                            }).collect()
                        }>
                            <Icon name="sort"/><span>{move || if sort.get() == "title" { "Title" } else { "Edited" }}</span>
                        </ActionMenu>
                    </Show>
                </div>
                {move || library.get().filter(|l| l.legacy > 0 && !notes_imported.get()).map(|lib| view! {
                    <LegacyImport
                        session=session_sv.get_value()
                        library=lib
                        on_done=Callback::new(move |text: String| { notice.set(text); reload.update(|n| *n += 1); })
                        on_error=Callback::new(move |text: String| error.set(text))
                    />
                })}
                {move || (!error.get().is_empty()).then(|| view! {
                    <div role="alert" class="error">
                        {error.get()}
                        <button class="text-button" on:click=move |_| { error.set(String::new()); reload.update(|n| *n += 1); }>"Reload"</button>
                    </div>
                })}
                {move || (!notice.get().is_empty()).then(|| view! {
                    <p role="status" class="notes-notice">
                        {notice.get()}
                        <button class="icon-button" aria-label="Dismiss" on:click=move |_| notice.set(String::new())><Icon name="close"/></button>
                    </p>
                })}
                <Show
                    when=move || scope.get() == "graph"
                    fallback=move || view! {
                        {move || (library.with(Option::is_none) && error.with(String::is_empty)).then(|| view! {
                            <p role="status">"Loading notes\u{2026}"</p>
                        })}
                        {move || {
                            let lib = library.get();
                            let list = rows.get();
                            lib.filter(|_| list.is_empty()).map(|lib| {
                                let q = query.get();
                                let s = scope.get();
                                let tagged = s.starts_with("tag:");
                                let icon = if !q.is_empty() { "search" } else if s == "favorites" { "star" } else if tagged { "tag" } else { "file" };
                                let heading = if !q.is_empty() { "No matching notes" } else if s == "favorites" { "No favorite notes" } else if tagged { "No notes with this tag" } else { "No notes yet" };
                                let body = if !q.is_empty() {
                                    "Search covers note names, headings and text.".to_owned()
                                } else if tagged {
                                    "Add the tag in a note\u{2019}s text or its tags property.".to_owned()
                                } else if s == "favorites" {
                                    "Star a note to find it here.".to_owned()
                                } else {
                                    let suffix = current_folder_path.get().map(|p| format!(" \u{203a} {p}")).unwrap_or_default();
                                    format!("Notes are Markdown files in Files \u{203a} {}{suffix}.", lib.path)
                                };
                                let show_new = q.is_empty() && s != "favorites" && !tagged;
                                view! {
                                    <div class="empty-state">
                                        <span class="empty-icon"><Icon name=icon/></span>
                                        <h2>{heading}</h2>
                                        <p>{body}</p>
                                        {show_new.then(|| view! {
                                            <button class="primary-button" disabled=move || busy.get() on:click=move |_| new_note()><Icon name="plus"/>"New note"</button>
                                        })}
                                    </div>
                                }
                            })
                        }}
                        {
                            let folder_meta = Memo::new(move |_| {
                                library.with(|l| {
                                    let root_id = l.as_ref().map(|l| l.root_id.clone()).unwrap_or_default();
                                    let names: HashMap<String, String> = l
                                        .as_ref()
                                        .map(|l| l.folders.iter().map(|f| (f.id.clone(), f.name.clone())).collect())
                                        .unwrap_or_default();
                                    (root_id, names)
                                })
                            });
                            view! {
                                <For
                                    each=move || rows.with(|list| list.iter().map(|n| n.id.clone()).collect::<Vec<_>>())
                                    key=|id| id.clone()
                                    let:note_id
                                >
                                    {
                                        let id_row = note_id.clone();
                                        let note_sig = Memo::new(move |_| {
                                            rows.with(|list| list.iter().find(|n| n.id == id_row).cloned())
                                        });
                                        let id_sel = note_id.clone();
                                        let id_click = note_id.clone();
                                        let action_label = Signal::derive(move || {
                                            note_sig.with(|n| format!("Actions for {}", n.as_ref().map(|n| n.title.as_str()).unwrap_or("")))
                                        });
                                        view! {
                                            <div class="workspace-entry">
                                                <article class=move || format!("workspace-row {}", if open.with(|o| o.as_ref().is_some_and(|e| e.id == id_sel)) { "selected" } else { "" })>
                                                    <button class="workspace-row-content" on:click=move |_| {
                                                        if !open.with_untracked(|o| o.as_ref().is_some_and(|e| e.id == id_click)) {
                                                            open_note(id_click.clone());
                                                        }
                                                    }>
                                                        <strong>
                                                            {move || note_sig.with(|n| {
                                                                let t = n.as_ref().map(|n| n.title.as_str()).unwrap_or("");
                                                                if t.is_empty() { "Untitled".to_owned() } else { t.to_owned() }
                                                            })}
                                                            {move || note_sig.with(|n| n.as_ref().is_some_and(|n| n.favorite)).then(|| view! { <Icon name="star" class="row-favorite"/> })}
                                                        </strong>
                                                        <small class="note-excerpt">
                                                            {move || note_sig.with(|n| {
                                                                let e = n.as_ref().map(|n| n.excerpt.as_str()).unwrap_or("");
                                                                if e.is_empty() { "No additional text".to_owned() } else { e.to_owned() }
                                                            })}
                                                        </small>
                                                        <span class="row-meta">
                                                            <span>{move || note_sig.with(|n| edited(n.as_ref().map(|n| n.modified_at).unwrap_or(0.0)))}</span>
                                                            {move || {
                                                                let parent_id = note_sig.with(|n| n.as_ref().map(|n| n.parent_id.clone()).unwrap_or_default());
                                                                folder_meta.with(|(root_id, names)| {
                                                                    (!parent_id.is_empty() && parent_id != *root_id).then(|| {
                                                                        let parent_label = names.get(&parent_id).cloned().unwrap_or_else(|| "Notes".into());
                                                                        view! { <span><Icon name="folder"/>{parent_label}</span> }
                                                                    })
                                                                })
                                                            }}
                                                        </span>
                                                    </button>
                                                    <div class="workspace-row-tools">
                                                        <ActionMenu label=action_label class="ui-file-actions" actions=move |_| {
                                                            note_sig.with_untracked(|n| n.clone()).map(build_note_actions).unwrap_or_default()
                                                        }><Icon name="more"/></ActionMenu>
                                                    </div>
                                                </article>
                                            </div>
                                        }
                                    }
                                </For>
                            }
                        }
                        {move || library.with(|l| l.as_ref().is_some_and(|l| l.truncated)).then(|| view! {
                            <p class="muted small">"This folder is large; only the first notes are shown. Use search to find others."</p>
                        })}
                    }
                >
                    <NoteGraph
                        open_id=Signal::derive(move || open.with(|o| o.as_ref().map(|e| e.id.clone())))
                        version=Signal::derive(move || reload.get() + graph_epoch.get())
                        on_open=Callback::new(move |id: String| {
                            if !open.with_untracked(|o| o.as_ref().is_some_and(|e| e.id == id)) {
                                open_note(id);
                            }
                        })
                    />
                </Show>
            </div>

            {move || pane.get().map(|current_pane| {
                let current_pane_submit = current_pane.clone();
                view! {
                    <NotesPane
                        pane=current_pane
                        folders=sorted_folders
                        library=library
                        busy=busy
                        on_close=Callback::new(move |_| pane.set(None))
                        on_submit=Callback::new(move |value: String| {
                            let p = current_pane_submit.clone();
                            spawn_local(async move {
                                busy.set(true);
                                error.set(String::new());
                                let done_msg = match &p {
                                    NotesPaneKind::Move(_) => format!("Moved to {}.", folder_name(&value)),
                                    _ => String::new(),
                                };
                                let mut updated = String::new();
                                let result: Result<(), String> = async {
                                    let conn = live.connection.get_untracked().ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
                                    match p {
                                        NotesPaneKind::Folder { parent_id } => {
                                            reducer(|tx| conn.reducers.create_folder_then(crate::random_uuid(), parent_id, value, replied(tx))).await?;
                                        }
                                        NotesPaneKind::RenameFolder(folder) => {
                                            let entry = fetch_detail(&conn, &folder.id).await?.entry;
                                            // Notes inside move with the folder: links naming their path follow.
                                            let before = index.get_untracked();
                                            let old_prefix = format!("{}/", folder.path);
                                            let new_folder = joined(folder.path.rfind('/').map(|i| folder.path[..i].to_owned()).unwrap_or_default(), &value);
                                            let moves: Vec<(String, String)> = before.notes.iter().filter_map(|n| n.path.strip_prefix(&old_prefix).map(|rest| (n.id.clone(), format!("{new_folder}/{rest}")))).collect();
                                            let mut sources = Vec::new();
                                            if moves.len() <= 200 {
                                                for (id, _) in &moves {
                                                    for source in note_links::backlink_ids(id).await {
                                                        if !sources.contains(&source) {
                                                            sources.push(source);
                                                        }
                                                    }
                                                }
                                            }
                                            reducer(|tx| conn.reducers.rename_entry_then(entry.id, value, entry.revision, replied(tx))).await?;
                                            if moves.len() <= 200 {
                                                updated = relink(moves, sources, before).await;
                                            }
                                        }
                                        NotesPaneKind::RenameNote(note) => {
                                            let entry = fetch_detail(&conn, &note.id).await?.entry;
                                            let next_name = if has_md_ext(&value) { value } else { format!("{value}.md") };
                                            let (before, sources) = (index.get_untracked(), note_links::backlink_ids(&entry.id).await);
                                            let new_path = joined(folder_path(&entry.parent_id), &next_name);
                                            reducer(|tx| conn.reducers.rename_entry_then(entry.id.clone(), next_name, entry.revision, replied(tx))).await?;
                                            updated = relink(vec![(entry.id.clone(), new_path)], sources, before).await;
                                            if open.with_untracked(|o| o.as_ref().is_some_and(|e| e.id == entry.id)) {
                                                open.set(Some(fetch_detail(&conn, &entry.id).await?.entry));
                                            }
                                        }
                                        NotesPaneKind::Move(note) => {
                                            let entry = fetch_detail(&conn, &note.id).await?.entry;
                                            let (before, sources) = (index.get_untracked(), note_links::backlink_ids(&entry.id).await);
                                            let new_path = joined(folder_path(&value), &entry.name);
                                            reducer(|tx| conn.reducers.move_entry_then(entry.id.clone(), value, entry.revision, None, replied(tx))).await?;
                                            updated = relink(vec![(entry.id.clone(), new_path)], sources, before).await;
                                            if open.with_untracked(|o| o.as_ref().is_some_and(|e| e.id == entry.id)) {
                                                open.set(Some(fetch_detail(&conn, &entry.id).await?.entry));
                                            }
                                        }
                                    }
                                    Ok(())
                                }
                                .await;
                                match result {
                                    Ok(()) => {
                                        let message = format!("{done_msg}{updated}");
                                        if !message.trim().is_empty() {
                                            notice.set(message.trim().to_owned());
                                        }
                                        reload.update(|n| *n += 1);
                                        pane.set(None);
                                    }
                                    Err(e) => error.set(error_message(&e)),
                                }
                                busy.set(false);
                            });
                        })
                    />
                }
            })}

            {move || confirm.get().map(|c| {
                let target = StoredValue::new(c.target.clone());
                view! {
                    <Dialog title=c.title on_close=Callback::new(move |_| confirm.set(None))>
                        <p>{c.text.clone()}</p>
                        {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                        <div class="dialog-actions">
                            <button class="text-button" on:click=move |_| confirm.set(None)>"Cancel"</button>
                            <button class="danger-button" disabled=move || busy.get() on:click=move |_| {
                                let target = target.get_value();
                                spawn_local(async move {
                                    busy.set(true);
                                    error.set(String::new());
                                    let result: Result<(), String> = async {
                                        let conn = live.connection.get_untracked().ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
                                        match target {
                                            ConfirmTarget::Note(note) => {
                                                let entry = fetch_detail(&conn, &note.id).await?.entry;
                                                reducer(|tx| conn.reducers.trash_entry_then(entry.id, entry.revision, replied(tx))).await?;
                                                if open.with_untracked(|o| o.as_ref().is_some_and(|e| e.id == note.id)) {
                                                    open.set(None);
                                                }
                                            }
                                            ConfirmTarget::Folder(folder) => {
                                                let entry = fetch_detail(&conn, &folder.id).await?.entry;
                                                reducer(|tx| conn.reducers.trash_entry_then(entry.id, entry.revision, replied(tx))).await?;
                                                if scope.get_untracked() == folder.id {
                                                    scope.set("all".into());
                                                }
                                            }
                                        }
                                        Ok(())
                                    }
                                    .await;
                                    match result {
                                        Ok(()) => {
                                            notice.set("Moved to deleted files.".into());
                                            reload.update(|n| *n += 1);
                                            confirm.set(None);
                                        }
                                        Err(e) => error.set(error_message(&e)),
                                    }
                                    busy.set(false);
                                });
                            }>{c.action.clone()}</button>
                        </div>
                    </Dialog>
                }
            })}
        </WorkspaceShell>
    }
}

/// New folder, rename and move right-hand editing pane (`NotesPane`).
#[component]
fn NotesPane(
    pane: NotesPaneKind,
    folders: Memo<Vec<NotesFolder>>,
    library: RwSignal<Option<NotesLibrary>>,
    busy: RwSignal<bool>,
    on_close: Callback<()>,
    on_submit: Callback<String>,
) -> impl IntoView {
    let initial = match &pane {
        NotesPaneKind::Folder { .. } => String::new(),
        NotesPaneKind::RenameFolder(f) => f.name.clone(),
        NotesPaneKind::RenameNote(n) => strip_md_ext(&n.name),
        NotesPaneKind::Move(n) => n.parent_id.clone(),
    };
    let title = match &pane {
        NotesPaneKind::Folder { .. } => "New folder",
        NotesPaneKind::RenameFolder(_) => "Rename folder",
        NotesPaneKind::RenameNote(_) => "Rename note",
        NotesPaneKind::Move(_) => "Move note",
    };
    let submit_label = match &pane {
        NotesPaneKind::Folder { .. } => "Create folder",
        NotesPaneKind::Move(_) => "Move here",
        _ => "Save",
    };
    let is_move = matches!(pane, NotesPaneKind::Move(_));
    let is_rename_note = matches!(pane, NotesPaneKind::RenameNote(_));
    let value = RwSignal::new(initial.clone());
    let initial_sv = StoredValue::new(initial);

    let folder_options = Signal::derive(move || {
        let root_id = library.with(|l| l.as_ref().map(|l| l.root_id.clone()).unwrap_or_default());
        let mut opts = vec![(root_id, "Notes (top level)".to_owned())];
        for folder in folders.get() {
            let label = folder.path.split('/').collect::<Vec<_>>().join(" \u{203a} ");
            opts.push((folder.id, label));
        }
        opts
    });

    view! {
        <EditorPane title=title on_close=on_close>
            <form on:submit=move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                let trimmed = value.get_untracked().trim().to_owned();
                if !trimmed.is_empty() {
                    { on_submit.try_run(trimmed); };
                }
            }>
                {if is_move {
                    view! {
                        <label>"Folder"
                            <ReactiveSelect options=folder_options value=value/>
                        </label>
                    }.into_any()
                } else {
                    view! {
                        <label>"Name"
                            <input autofocus required maxlength="255" prop:value=move || value.get() on:input=move |e| value.set(event_target_value(&e))/>
                            {is_rename_note.then(|| view! { <small class="field-hint">"Saved as "{move || file_name(&value.get())}</small> })}
                        </label>
                    }.into_any()
                }}
                <div class="dialog-actions">
                    <button type="button" class="text-button">"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get() || value.get().trim().is_empty() || value.get() == initial_sv.get_value()>
                        {move || if busy.get() { "Saving\u{2026}" } else { submit_label }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}

#[derive(Deserialize)]
struct NotebookList {
    items: Vec<Collection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPage {
    items: Vec<LegacyRowRef>,
    #[serde(default)]
    next_cursor: String,
}

#[derive(Deserialize)]
struct LegacyRowRef {
    id: String,
}

#[derive(Deserialize)]
struct LegacyNote {
    #[serde(default)]
    content: String,
    data: LegacyNoteData,
}

#[derive(Deserialize)]
struct LegacyNoteData {
    #[serde(default)]
    title: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    favorite: bool,
}

/// Copies legacy pre-Files notes once into the Notes folder (`LegacyImport`).
#[component]
fn LegacyImport(
    session: BrowserSession,
    library: NotesLibrary,
    on_done: Callback<String>,
    on_error: Callback<String>,
) -> impl IntoView {
    let live = expect_context::<Live>();
    let shared_prefs = context().preferences;
    let progress: RwSignal<Option<String>> = RwSignal::new(None);
    let session = StoredValue::new(session);
    let library = StoredValue::new(library);

    let save_imported = move || {
        let rev = shared_prefs.value.with_untracked(|p| p.as_ref().map(|p| p.revision).unwrap_or(0));
        let sess = session.get_value();
        async move { save_preferences(json!({ "revision": rev, "notesImported": true }), rev, &sess).await }
    };

    let copy = move || {
        progress.set(Some("Copying\u{2026}".into()));
        let sess = session.get_value();
        let lib = library.get_value();
        spawn_local(async move {
            let Some(conn) = live.connection.get_untracked() else {
                progress.set(None);
                { on_error.try_run("Notes are reconnecting. Try again in a moment.".into()); };
                return;
            };
            let result: Result<u64, String> = async {
                let books = api::get::<NotebookList>("/api/v1/notes/notebooks").await.map_err(|e| e.message)?;
                let mut copied = 0u64;
                for book in books.items.into_iter().filter(|b| b.role == "owner") {
                    let cleaned: String = book
                        .name
                        .chars()
                        .map(|c| if (c as u32) < 0x20 || c == '/' || c == '\\' { ' ' } else { c })
                        .collect();
                    let folder_name: String = {
                        let t: String = cleaned.trim().chars().take(200).collect();
                        if t.is_empty() { "Notebook".into() } else { t }
                    };
                    let folder = match lib.folders.iter().find(|r| r.parent_id == lib.root_id && r.name.to_lowercase() == folder_name.to_lowercase()) {
                        Some(f) => f.id.clone(),
                        None => {
                            let id = crate::random_uuid();
                            reducer(|tx| conn.reducers.create_folder_then(id.clone(), lib.root_id.clone(), folder_name.clone(), replied(tx))).await?;
                            id
                        }
                    };
                    let mut taken = HashSet::new();
                    let mut cursor = String::new();
                    loop {
                        let qs = search_params(&[("collection", book.id.clone()), ("cursor", cursor.clone())]);
                        let page = api::get::<LegacyPage>(&format!("/api/v1/notes/items?{qs}")).await.map_err(|e| e.message)?;
                        for row in page.items {
                            let item = api::get::<LegacyNote>(&format!("/api/v1/notes/items/{}", row.id)).await.map_err(|e| e.message)?;
                            let first_line = item.content.split('\n').next().unwrap_or("");
                            let has_same_heading = !item.data.title.is_empty()
                                && first_line.strip_prefix('#').is_some_and(|r| r.starts_with(char::is_whitespace) && r.trim() == item.data.title);
                            let heading = if !item.data.title.is_empty() && !has_same_heading {
                                format!("# {}\n\n", item.data.title)
                            } else {
                                String::new()
                            };
                            let front = if !item.data.tags.is_empty() {
                                let tags_json: Vec<String> = item.data.tags.iter().map(|t| serde_json::to_string(t).unwrap_or_default()).collect();
                                format!("---\ntags: [{}]\n---\n", tags_json.join(", "))
                            } else {
                                String::new()
                            };
                            let stem = if item.data.title.is_empty() { "Untitled" } else { &item.data.title };
                            let name = unique_name(&file_name(stem), &taken);
                            taken.insert(name.to_lowercase());
                            let full_content = format!("{front}{heading}{}", item.content);
                            let id = write_text_file(
                                &sess,
                                TextFile { name: &name, parent_id: &folder, content: &full_content, media_type: "text/markdown", file_id: None, expected_revision: None },
                            )
                            .await
                            .map_err(|e| e.message)?;
                            if item.data.favorite {
                                let _ = reducer(|tx| conn.reducers.favorite_entry_then(id, true, 1, replied(tx))).await;
                            }
                            copied += 1;
                            progress.set(Some(format!("Copied {copied} of {}\u{2026}", lib.legacy)));
                        }
                        if page.next_cursor.is_empty() {
                            break;
                        }
                        cursor = page.next_cursor;
                    }
                }
                save_imported().await.map_err(|e| e.message)?;
                Ok(copied)
            }
            .await;
            match result {
                Ok(copied) => { on_done.try_run(format!("Copied {copied} {} into Files \u{203a} {}.", if copied == 1 { "note" } else { "notes" }, lib.path)); },
                Err(e) => {
                    progress.set(None);
                    { on_error.try_run(error_message(&e)); };
                }
            }
        });
    };

    let legacy_count = library.with_value(|l| l.legacy);
    let lib_path = library.with_value(|l| l.path.clone());
    let prompt = format!(
        "{legacy_count} {} still in the old notes storage. Copy them into Files \u{203a} {lib_path} as Markdown files, one folder per notebook.",
        if legacy_count == 1 { "note is" } else { "notes are" }
    );
    let prompt = StoredValue::new(prompt);

    view! {
        <div class="notes-legacy" role="status">
            <p>{move || progress.get().unwrap_or_else(|| prompt.get_value())}</p>
            {move || progress.with(Option::is_none).then(|| view! {
                <button class="primary-button" on:click=move |_| copy()>"Copy into Files"</button>
                <button class="text-button" on:click=move |_| {
                    spawn_local(async move {
                        if save_imported().await.is_ok() {
                            { on_done.try_run("The old notes stay available for export in Notes settings.".into()); };
                        }
                    });
                }>"Not now"</button>
            })}
        </div>
    }
}

/// Full Markdown text editor for Notes (`text-editor.tsx`), extending `files::editor::TextEditor`
/// with folder context, `start_in_preview`, `on_saved`, and note actions.
#[component]
fn NoteTextEditor(
    entry: Signal<FileEntry>,
    session: BrowserSession,
    #[prop(into)] context: Signal<Option<String>>,
    start_in_preview: bool,
    on_close: Callback<()>,
    on_saved: Callback<(u32, String)>,
    actions: Callback<(), Vec<MenuAction>>,
    links: NoteLinks,
    index: RwSignal<Arc<NoteIndex>>,
    anchor: RwSignal<Option<(String, String)>>,
    #[prop(into)] refresh: Signal<u32>,
    on_mode: Callback<bool>,
    children: Children,
) -> impl IntoView {
    use wasm_bindgen::JsCast;
    let live = expect_context::<Live>();
    let initial = entry.get_untracked();
    let text = RwSignal::new(String::new());
    let saved = RwSignal::new(String::new());
    let status = RwSignal::new("Loading\u{2026}");
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let ready = RwSignal::new(false);
    let read_only = RwSignal::new(true);
    let preview = RwSignal::new(start_in_preview);
    let discard = RwSignal::new(false);
    let recovery: RwSignal<Option<String>> = RwSignal::new(None);
    let conflict: RwSignal<Option<(String, FileEntry)>> = RwSignal::new(None);
    let modified = RwSignal::new(initial.modified_at);
    let revision = RwSignal::new(initial.revision);
    let saving = RwSignal::new(false);
    let history: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let future: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let input = NodeRef::<html::Textarea>::new();
    let key = StoredValue::new(format!("dar.file-draft.{}.{}", session.user.id, initial.id));
    let session = StoredValue::new(session);
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }

    let markdown = Memo::new(move |_| has_md_ext(&entry.with(|e| e.name.clone())));
    let dirty = Memo::new(move |_| text.with(|t| saved.with(|s| t != s)));

    // Keep revision synchronized if the open note is renamed or moved via the Notes pane.
    Effect::new(move |_| {
        let rev = entry.with(|e| e.revision);
        if ready.get_untracked() && !saving.get_untracked() && rev > revision.get_untracked() {
            revision.set(rev);
        }
    });

    // Initial load.
    {
        let alive = alive.clone();
        let entry_id = initial.id.clone();
        spawn_local(async move {
            let conn = match live.connection.get_untracked() {
                Some(c) => c,
                None => {
                    if alive.load(Ordering::Relaxed) {
                        error.set("Notes are reconnecting. Try again in a moment.".into());
                    }
                    return;
                }
            };
            let detail = match fetch_detail(&conn, &entry_id).await {
                Ok(d) => d,
                Err(e) => {
                    if alive.load(Ordering::Relaxed) {
                        error.set(e);
                    }
                    return;
                }
            };
            if !editable_text(&detail.entry) || detail.entry.trashed {
                if alive.load(Ordering::Relaxed) {
                    error.set("This file cannot be edited as text.".into());
                }
                return;
            }
            match read_file_content(&detail.entry.id).await {
                Ok(content) => {
                    if !alive.load(Ordering::Relaxed) {
                        return;
                    }
                    revision.set(detail.entry.revision);
                    modified.set(detail.entry.modified_at);
                    text.set(content.clone());
                    saved.set(content.clone());
                    let read = detail.role == "read";
                    read_only.set(read);
                    ready.set(true);
                    status.set(if read { "Read only" } else { "Saved" });
                    if let Some(draft) = session_storage()
                        .and_then(|s| s.get_item(&key.get_value()).ok().flatten())
                        .filter(|d| *d != content)
                    {
                        recovery.set(Some(draft));
                    }
                }
                Err(e) => {
                    if alive.load(Ordering::Relaxed) {
                        error.set(e);
                    }
                }
            }
        });
    }

    // Recovery draft in sessionStorage + beforeunload protection.
    Effect::new(move |_| {
        let (t, d) = (text.get(), dirty.get());
        if !ready.get() || read_only.get() || recovery.with(Option::is_some) {
            return;
        }
        if let Some(s) = session_storage() {
            let result = if d { s.set_item(&key.get_value(), &t) } else { s.remove_item(&key.get_value()) };
            if result.is_err() {
                error.set("Recovery storage is unavailable. Keep this editor open until the file is saved.".into());
            }
        }
    });
    listen(&window(), "beforeunload", false, move |event| {
        if dirty.get_untracked() {
            event.prevent_default();
        }
    });

    let alive_compare = alive.clone();
    let compare = move || {
        let alive = alive_compare.clone();
        spawn_local(async move {
            let id = entry.with_untracked(|e| e.id.clone());
            let outcome: Result<(String, FileEntry), String> = async {
                let conn = live.connection.get_untracked().ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
                let detail = fetch_detail(&conn, &id).await?;
                let content = read_file_content(&id).await.map_err(|_| "Saved version unavailable.".to_owned())?;
                Ok((content, detail.entry))
            }
            .await;
            if !alive.load(Ordering::Relaxed) {
                return;
            }
            match outcome {
                Ok(val) => conflict.set(Some(val)),
                Err(e) => error.set(e),
            }
        });
    };

    let alive_save = alive.clone();
    let compare_for_save = compare.clone();
    let save = move || {
        if !ready.get_untracked()
            || read_only.get_untracked()
            || saving.get_untracked()
            || recovery.with_untracked(Option::is_some)
            || conflict.with_untracked(Option::is_some)
            || !dirty.get_untracked()
        {
            return;
        }
        let content = text.get_untracked();
        let expected = revision.get_untracked();
        saving.set(true);
        status.set("Saving\u{2026}");
        error.set(String::new());
        let alive = alive_save.clone();
        let compare_now = compare_for_save.clone();
        spawn_local(async move {
            let (cur, sess) = (entry.get_untracked(), session.get_value());
            let result = write_text_file(
                &sess,
                TextFile {
                    name: &cur.name,
                    parent_id: &cur.parent_id,
                    content: &content,
                    media_type: &cur.media_type,
                    file_id: Some(&cur.id),
                    expected_revision: Some(expected),
                },
            )
            .await;
            if alive.load(Ordering::Relaxed) {
                match result {
                    Ok(_) => {
                        revision.set(expected + 1);
                        saved.set(content.clone());
                        modified.set(js_sys::Date::now());
                        status.set("Saved");
                        { on_saved.try_run((expected + 1, content)); };
                    }
                    Err(e) => {
                        error.set(e.message.clone());
                        status.set("Save failed");
                        if [409, 412].contains(&e.status) {
                            compare_now();
                        }
                    }
                }
                saving.set(false);
            }
        });
    };

    // Autosave 900 ms after typing stops.
    let typed = RwSignal::new(0u32);
    let save_for_auto = save.clone();
    Effect::new(move |_| {
        text.track();
        saved.track();
        if !dirty.get() || !error.with(String::is_empty) || !ready.get() || recovery.with(Option::is_some) || conflict.with(Option::is_some) {
            return;
        }
        let id = typed.get_untracked() + 1;
        typed.set(id);
        let save_now = save_for_auto.clone();
        spawn_local(async move {
            crate::sleep(900).await;
            if typed.try_get_untracked() == Some(id) {
                save_now();
            }
        });
    });

    let change_text = move |value: String| {
        history.update(|h| {
            h.push(text.get_untracked());
            if h.len() > 100 {
                h.remove(0);
            }
        });
        future.set(Vec::new());
        if error.get_untracked() == "Put the cursor on a line to link to it." {
            error.set(String::new());
        }
        text.set(value);
        status.set("Unsaved");
    };

    // Other notes' link updates may have rewritten this one: a clean editor takes the new text.
    let alive_refresh = alive.clone();
    Effect::new(move |previous: Option<u32>| {
        let tick = refresh.get();
        if previous.is_some_and(|p| p != tick) && ready.get_untracked() && !dirty.get_untracked() && !saving.get_untracked() {
            let alive = alive_refresh.clone();
            let id = entry.with_untracked(|e| e.id.clone());
            spawn_local(async move {
                let Some(conn) = live.connection.get_untracked() else { return };
                let (Ok(detail), Ok(content)) = (fetch_detail(&conn, &id).await, read_file_content(&id).await) else { return };
                if alive.load(Ordering::Relaxed) && !dirty.get_untracked() && detail.entry.revision > revision.get_untracked() {
                    revision.set(detail.entry.revision);
                    modified.set(detail.entry.modified_at);
                    text.set(content.clone());
                    saved.set(content);
                }
            });
        }
        tick
    });

    let headings = Memo::new(move |_| text.with(|t| dar_notes::facts(t).headings.into_iter().map(|h| (h.level, h.text, h.offset)).collect::<Vec<_>>()));
    let section = NodeRef::<html::Section>::new();
    // Shows a heading (`Heading`, `Heading#Sub`) or block (`^id`): scrolled to in the preview,
    // selected in the source.
    let reveal = move |fragment: String| {
        let block = fragment.strip_prefix('^').map(str::to_owned);
        let wanted = dar_notes::slug(fragment.rsplit('#').next().unwrap_or(&fragment));
        if preview.get_untracked() {
            let Some(root) = section.get_untracked() else { return };
            let selector = match &block {
                Some(id) => format!("[data-block=\"{}\"]", id.replace('"', "")),
                None => format!("[data-anchor=\"{}\"]", wanted.replace('"', "")),
            };
            if let Ok(Some(found)) = root.query_selector(&selector) {
                let options = web_sys::ScrollIntoViewOptions::new();
                options.set_block(web_sys::ScrollLogicalPosition::Start);
                found.scroll_into_view_with_scroll_into_view_options(&options);
            }
            return;
        }
        let source = text.get_untracked();
        let offset = match &block {
            Some(id) => {
                let mut at = 0;
                source.split_inclusive('\n').find_map(|line| {
                    let start = at;
                    at += line.len();
                    (dar_notes::block_id(line.trim_end()) == Some(id.as_str())).then_some(start)
                })
            }
            None => headings.get_untracked().into_iter().find(|(_, t, _)| dar_notes::slug(t) == wanted).map(|(_, _, o)| o),
        };
        if let (Some(offset), Some(field)) = (offset, input.get_untracked()) {
            let caret = utf16_len(&source[..offset]);
            let _ = field.focus();
            let _ = field.set_selection_range(caret, caret);
            let _ = field.blur();
            let _ = field.focus();
        }
    };
    Effect::new(move |_| {
        let Some((id, fragment)) = anchor.get() else { return };
        if !ready.get() || id != entry.with_untracked(|e| e.id.clone()) {
            return;
        }
        anchor.set(None);
        request_animation_frame(move || reveal(fragment));
    });

    let toggle_task = Callback::new(move |offset: usize| {
        if read_only.get_untracked() || !ready.get_untracked() {
            return;
        }
        let source = text.get_untracked();
        let replacement = match source.get(offset..offset + 3) {
            Some("[ ]") => "[x]",
            Some("[x]") | Some("[X]") => "[ ]",
            _ => return,
        };
        let mut next = source.clone();
        next.replace_range(offset..offset + 3, replacement);
        change_text(next);
    });
    // Hover/focus preview card for resolved note links in the preview pane.
    let hover_preview: RwSignal<Option<note_links::PreviewTarget>> = RwSignal::new(None);
    let hover_open_seq = RwSignal::new(0u32);
    let hover_close_seq = RwSignal::new(0u32);
    let close_hover_now = Callback::new(move |()| {
        hover_open_seq.try_update(|n| *n += 1);
        hover_close_seq.try_update(|n| *n += 1);
        hover_preview.try_set(None);
    });
    let cancel_hover_close = Callback::new(move |()| {
        hover_close_seq.try_update(|n| *n += 1);
    });
    let alive_hover_close = alive.clone();
    let schedule_hover_close = Callback::new(move |()| {
        let Some(ticket) = hover_close_seq.try_update(|n| {
            *n += 1;
            *n
        }) else {
            return;
        };
        let alive = alive_hover_close.clone();
        spawn_local(async move {
            crate::sleep(180).await;
            if alive.load(Ordering::Relaxed) && hover_close_seq.try_get_untracked() == Some(ticket) {
                hover_preview.try_set(None);
            }
        });
    });
    let alive_hover_open = alive.clone();
    let schedule_hover_open = move |target_el: web_sys::Element| {
        if !preview.get_untracked() {
            return;
        }
        let Some(next) = index.with_untracked(|ix| note_links::preview_target_from_element(&target_el, ix)) else {
            return;
        };
        hover_close_seq.try_update(|n| *n += 1);
        let same = hover_preview.try_with_untracked(|cur| cur.as_ref().is_some_and(|c| c.id == next.id && c.fragment == next.fragment)) == Some(true);
        if same {
            return;
        }
        let Some(ticket) = hover_open_seq.try_update(|n| {
            *n += 1;
            *n
        }) else {
            return;
        };
        let alive = alive_hover_open.clone();
        spawn_local(async move {
            crate::sleep(500).await;
            if alive.load(Ordering::Relaxed)
                && preview.try_get_untracked() == Some(true)
                && hover_open_seq.try_get_untracked() == Some(ticket)
            {
                hover_preview.try_set(Some(next));
            }
        });
    };
    Effect::new(move |_| {
        if !preview.get() {
            close_hover_now.try_run(());
        }
    });

    let alive_copy = alive.clone();
    let copy_block_link = Callback::new(move |()| {
        let Some(field) = input.get_untracked() else { return };
        let source = text.get_untracked();
        let caret16 = field.selection_start().ok().flatten().unwrap_or(0);
        let caret = byte_index(&source, caret16);
        let existing = dar_notes::facts(&source).blocks;
        let Some((block_id, updated)) = note_links::ensure_block_id(&source, caret, || {
            for _ in 0..8 {
                let candidate = note_links::random_block_id();
                if !existing.iter().any(|b| b.eq_ignore_ascii_case(&candidate)) {
                    return candidate;
                }
            }
            note_links::random_block_id()
        }) else {
            notice.set(String::new());
            error.set("Put the cursor on a line to link to it.".into());
            return;
        };
        if error.get_untracked() == "Put the cursor on a line to link to it." {
            error.set(String::new());
        }
        if let Some(next) = updated {
            field.set_value(&next);
            let _ = field.set_selection_range(caret16, caret16);
            change_text(next);
        }
        let note_id = entry.with_untracked(|e| e.id.clone());
        let target = index
            .with_untracked(|ix| ix.position(&note_id).map(|i| ix.vault.wiki_target(i)))
            .unwrap_or_else(|| strip_md_ext(&entry.with_untracked(|e| e.name.clone())));
        let link = format!("[[{target}#^{block_id}]]");
        let alive = alive_copy.clone();
        spawn_local(async move {
            let copied = note_links::copy_to_clipboard(&link).await.is_ok();
            if alive.load(Ordering::Relaxed) {
                notice.try_set(if copied { "Block link copied.".to_owned() } else { link });
            }
        });
    });

    let preview_links = NoteLinks {
        toggle: Some(toggle_task),
        follow: links.follow.map(|f| {
            Callback::new(move |args| {
                close_hover_now.try_run(());
                f.try_run(args);
            })
        }),
        ..links
    };

    // `[[` and `#` suggestions while writing.
    let suggest_id = unique_id();
    let listbox_id = suggest_id.clone();
    let suggest: RwSignal<Option<Trigger>> = RwSignal::new(None);
    let choices: RwSignal<Vec<Suggestion>> = RwSignal::new(Vec::new());
    let active = RwSignal::new(0usize);
    let point: RwSignal<Option<(f64, f64)>> = RwSignal::new(None);
    let update_suggestions = move || {
        let Some(field) = input.get_untracked() else { return };
        let caret16 = field.selection_start().ok().flatten().unwrap_or(0);
        if field.selection_end().ok().flatten() != Some(caret16) {
            choices.set(Vec::new());
            return;
        }
        let source = text.get_untracked();
        let caret = byte_index(&source, caret16);
        let found = note_links::trigger(&source, caret);
        let list = found.as_ref().map(|t| index.with_untracked(|ix| note_links::suggestions(ix, t, &entry.with_untracked(|e| e.id.clone())))).unwrap_or_default();
        if !list.is_empty() {
            point.set(note_links::caret_point(&field, caret16));
        }
        if suggest.get_untracked() != found {
            active.set(0);
        }
        active.update(|a| *a = (*a).min(list.len().saturating_sub(1)));
        suggest.set(found);
        choices.set(list);
    };
    // Text inserted at the caret (pasted files' embeds, templates, slash commands).
    let alive_insert = alive.clone();
    let insert_at_caret = move |insert: String| {
        if !alive_insert.load(Ordering::Relaxed) {
            return;
        }
        let Some(field) = input.get_untracked() else {
            change_text(format!("{}{insert}", text.get_untracked()));
            return;
        };
        let source = text.get_untracked();
        let start = byte_index(&source, field.selection_start().ok().flatten().unwrap_or(u32::MAX));
        let end = byte_index(&source, field.selection_end().ok().flatten().unwrap_or(u32::MAX)).max(start);
        let next = format!("{}{insert}{}", &source[..start], &source[end..]);
        let caret16 = utf16_len(&next[..start + insert.len()]);
        field.set_value(&next);
        let _ = field.set_selection_range(caret16, caret16);
        change_text(next);
    };
    let insert_at_caret = StoredValue::new(insert_at_caret);
    let pick = Callback::new(move |i: usize| {
        let (Some(trigger), Some(choice), Some(field)) = (suggest.get_untracked(), choices.with_untracked(|c| c.get(i).cloned()), input.get_untracked()) else { return };
        let source = text.get_untracked();
        let caret = byte_index(&source, field.selection_start().ok().flatten().unwrap_or(0));
        let (next, at) = note_links::accept(&source, caret, &trigger, &choice.insert);
        let caret16 = utf16_len(&next[..at]);
        // The field takes the text and caret at once, so typing that follows lands after the link.
        field.set_value(&next);
        let _ = field.set_selection_range(caret16, caret16);
        change_text(next);
        choices.set(Vec::new());
        suggest.set(None);
        if let Some(template) = choice.template {
            let title = strip_md_ext(&entry.with_untracked(|e| e.name.clone()));
            spawn_local(async move {
                match read_file_content(&template).await {
                    Ok(body) => {
                        let (date, time) = note_links::today();
                        let filled = note_links::fill_template(&body, &title, &date, &time);
                        insert_at_caret.with_value(|f| f(filled));
                    }
                    Err(e) => error.set(e),
                }
            });
        } else {
            // "Link to note" leaves the caret inside `[[]]`: the note list opens.
            update_suggestions();
        }
    });
    let attach_files = move |files: Vec<web_sys::File>| {
        if files.is_empty() || read_only.get_untracked() || !ready.get_untracked() {
            return;
        }
        status.set("Uploading\u{2026}");
        spawn_local(async move {
            let ix = index.get_untracked();
            let result = note_links::attach(live, &session.get_value(), &ix, files).await;
            match result {
                Ok(embeds) => insert_at_caret.with_value(|f| f(embeds)),
                Err(e) => {
                    error.set(e);
                    status.set("Unsaved");
                }
            }
        });
    };
    let file_list = |list: Option<web_sys::FileList>| -> Vec<web_sys::File> { list.map(|l| (0..l.length()).filter_map(|i| l.get(i)).collect()).unwrap_or_default() };

    // Outliner keys in lists: Tab/Shift+Tab indent, Alt+Shift+Up/Down move lines, Enter
    // continues the list. Outside lists Tab keeps moving focus.
    let outline_keys = move |e: &web_sys::KeyboardEvent| -> bool {
        let Some(field) = input.get_untracked() else { return false };
        if !markdown.get_untracked() || read_only.get_untracked() || e.ctrl_key() || e.meta_key() || e.is_composing() {
            return false;
        }
        let source = text.get_untracked();
        let start = byte_index(&source, field.selection_start().ok().flatten().unwrap_or(0));
        let end = byte_index(&source, field.selection_end().ok().flatten().unwrap_or(0));
        let edit = match e.key().as_str() {
            "Tab" if !e.alt_key() => crate::outline::indent(&source, start, end, e.shift_key()),
            "ArrowUp" | "ArrowDown" if e.alt_key() && e.shift_key() => crate::outline::move_lines(&source, start, end, e.key() == "ArrowUp"),
            "Enter" if !e.alt_key() && !e.shift_key() && start == end => crate::outline::continue_list(&source, start).map(|(t, c)| (t, c, c)),
            _ => None,
        };
        let Some((next, s, en)) = edit else { return false };
        let (s16, e16) = (utf16_len(&next[..s]), utf16_len(&next[..en]));
        field.set_value(&next);
        let _ = field.set_selection_range(s16, e16);
        change_text(next);
        true
    };
    let suggest_keys = move |e: web_sys::KeyboardEvent| {
        let count = choices.with_untracked(Vec::len);
        if count == 0 {
            if outline_keys(&e) {
                e.prevent_default();
            }
            return;
        }
        match e.key().as_str() {
            "ArrowDown" => active.update(|a| *a = (*a + 1) % count),
            "ArrowUp" => active.update(|a| *a = (*a + count - 1) % count),
            "Enter" | "Tab" => pick.run(active.get_untracked()),
            "Escape" => choices.set(Vec::new()),
            _ => return,
        }
        e.prevent_default();
        e.stop_propagation();
    };

    Effect::new(move |_| {
        if ready.get() && !read_only.get() && !preview.get()
            && let Some(i) = input.get()
        {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = i.focus_with_options(&options);
        }
    });


    let change = change_text;
    let format = Callback::new(move |(before, after): (&'static str, &'static str)| {
        let Some(field) = input.get_untracked() else { return };
        let (start, end) = (
            field.selection_start().ok().flatten().unwrap_or(0),
            field.selection_end().ok().flatten().unwrap_or(0),
        );
        let (next, s, e) = format_markdown(&text.get_untracked(), start, end, before, after);
        change(next);
        let field = send_wrapper::SendWrapper::new(field);
        request_animation_frame(move || {
            let _ = field.focus();
            let _ = field.set_selection_range(s, e);
        });
    });

    let phase = move || {
        if read_only.get() {
            "read"
        } else if matches!(status.get(), "Saving\u{2026}" | "Loading\u{2026}" | "Uploading\u{2026}") {
            "saving"
        } else if dirty.get() {
            if status.get() == "Save failed" { "error" } else { "dirty" }
        } else {
            "saved"
        }
    };

    let label = move || {
        let s = status.get();
        if matches!(s, "Loading\u{2026}" | "Saving\u{2026}" | "Uploading\u{2026}") || read_only.get() {
            s
        } else if dirty.get() {
            if s == "Save failed" { "Not saved" } else { "Unsaved changes" }
        } else {
            "All changes saved"
        }
    };

    let close = move || {
        if dirty.get_untracked() || saving.get_untracked() {
            discard.set(true);
        } else {
            { on_close.try_run(()); };
        }
    };

    let save_for_key = save.clone();
    let save_for_btn = save.clone();
    let save_for_retry = save.clone();
    let schedule_hover_open_focus = schedule_hover_open.clone();

    view! {
        <section node_ref=section
            class=move || if markdown.get() { "file-text-editor document-editor" } else { "file-text-editor document-editor is-plain-text" }
            aria-label=move || format!("Editor for {}", entry.with(|e| e.name.clone()))
            on:keydown=move |e| {
                if (e.ctrl_key() || e.meta_key()) && e.key().to_lowercase() == "s" {
                    e.prevent_default();
                    save_for_key();
                } else if e.key() == "Escape" && !e.default_prevented() {
                    e.prevent_default();
                    close();
                }
            }>
            <div class="workspace-editor-top">
                <button class="icon-button" aria-label="Close editor" title="Close editor" on:click=move |_| close()><Icon name="close"/></button>
                <span class="note-notebook">
                    {move || match context.get() {
                        Some(ctx) => view! { <Icon name="folder"/>{ctx} }.into_any(),
                        None => {
                            let e = entry.get();
                            view! { <FileTile kind=e.kind.clone() name=e.name.clone() media_type=e.media_type.clone()/>{file_type_label(&e.kind, &e.name, &e.media_type)} }.into_any()
                        }
                    }}
                </span>
                <span class=move || format!("note-status note-status-{}", phase()) role="status">
                    {move || (phase() == "saved").then(|| view! { <Icon name="check"/> })}
                    {label}
                </span>
                {move || (dirty.get() && !read_only.get()).then(|| {
                    let save_btn = save_for_btn.clone();
                    view! {
                        <button class="text-button" disabled=move || status.get() == "Saving\u{2026}" || conflict.with(Option::is_some) || recovery.with(Option::is_some) on:click=move |_| save_btn()>"Save now"</button>
                    }
                })}
                {move || markdown.get().then(|| view! {
                    <div class="segmented" role="group" aria-label="Editor mode">
                        <button aria-pressed=move || (!preview.get()).to_string() on:click=move |_| { preview.set(false); on_mode.try_run(false); }>"Write"</button>
                        <button aria-pressed=move || preview.get().to_string() on:click=move |_| { preview.set(true); on_mode.try_run(true); }>"Preview"</button>
                    </div>
                })}
                {move || (markdown.get() && !headings.with(Vec::is_empty)).then(|| view! {
                    <ActionMenu label="Outline" class="note-outline" actions=move |_| headings.get_untracked().into_iter().map(|(level, title, _)| {
                        let indent = "\u{2003}".repeat(usize::from(level.saturating_sub(1)).min(4));
                        MenuAction::plain(format!("{indent}{title}")).run(move || reveal(title.clone()))
                    }).collect()><Icon name="list"/></ActionMenu>
                })}
                <ActionMenu label="File options" actions=move |_| {
                    let mut list = actions.run(());
                    let has_sep = !list.is_empty();
                    let can_block_link = markdown.get_untracked() && !preview.get_untracked() && !read_only.get_untracked() && ready.get_untracked();
                    if can_block_link {
                        let mut item = MenuAction::new("Copy block link", "link").run(move || { copy_block_link.try_run(()); });
                        if has_sep {
                            item = item.separator();
                        }
                        list.push(item);
                    }
                    let id = entry.with_untracked(|e| e.id.clone());
                    let mut dl = MenuAction::new("Download", "download").href(format!("/api/v1/files/{id}/content")).download();
                    if has_sep && !can_block_link {
                        dl = dl.separator();
                    }
                    list.push(dl);
                    list
                }><Icon name="more"/></ActionMenu>
            </div>
            {move || discard.get().then(|| view! {
                <div class="draft-banner" role="alert">
                    <p>{if saving.get() { "A save is in progress. Keep the editor open until it completes." } else { "Unsaved changes are retained as a recovery draft in this tab." }}</p>
                    <button class="text-button" on:click=move |_| discard.set(false)>"Keep editing"</button>
                    <button class="text-button danger" disabled=move || saving.get() on:click=move |_| { on_close.try_run(()); }>"Close with recovery draft"</button>
                </div>
            })}
            {move || (!error.get().is_empty()).then(|| {
                let save_retry = save_for_retry.clone();
                let compare_retry = compare.clone();
                view! {
                    <div class="error-banner" role="alert">
                        <span>{error.get()}</span>
                        <button class="text-button" on:click=move |_| save_retry()>"Retry save"</button>
                        <button class="text-button" on:click=move |_| compare_retry()>"Review saved version"</button>
                    </div>
                }
            })}
            {move || (!notice.get().is_empty()).then(|| view! {
                <p role="status" class="notes-notice note-editor-notice">
                    <span>{notice.get()}</span>
                    <button type="button" class="icon-button" aria-label="Dismiss" on:click=move |_| notice.set(String::new())><Icon name="close"/></button>
                </p>
            })}
            {move || recovery.get().map(|draft| view! {
                <div class="draft-banner" role="alert">
                    <p>"A recovery draft is available."</p>
                    <button class="text-button" on:click=move |_| { change(draft.clone()); recovery.set(None); }>"Restore draft"</button>
                    <button class="text-button" on:click=move |_| { recovery.set(None); if let Some(s) = session_storage() { let _ = s.remove_item(&key.get_value()); } }>"Discard draft"</button>
                </div>
            })}
            {move || conflict.get().map(|(content, latest)| {
                let (content2, latest2) = (content.clone(), latest.clone());
                view! {
                    <div class="draft-banner" role="alert">
                        <p>"The saved file changed. Your text has been kept."</p>
                        <details><summary>"View saved version"</summary><pre>{content.clone()}</pre></details>
                        <button class="text-button" on:click=move |_| {
                            revision.set(latest2.revision);
                            modified.set(latest2.modified_at);
                            text.set(content2.clone());
                            saved.set(content2.clone());
                            conflict.set(None);
                            error.set(String::new());
                            status.set("Saved");
                        }>"Use saved version"</button>
                        <button class="text-button" on:click=move |_| {
                            revision.set(latest.revision);
                            conflict.set(None);
                            error.set(String::new());
                            status.set("Unsaved");
                        }>"Keep my version"</button>
                    </div>
                }
            })}
            <div class="note-document"
                on:mouseover=move |e: web_sys::MouseEvent| {
                    if !preview.get_untracked() {
                        return;
                    }
                    let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
                    if el.closest(".note-preview-card, .note-transclusion, .note-references").ok().flatten().is_some() {
                        return;
                    }
                    let Some(link) = el.closest("a.note-link:not(.is-unresolved)").ok().flatten() else { return };
                    if let Some(rel) = e.related_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                        && link.contains(Some(&rel))
                    {
                        return;
                    }
                    schedule_hover_open(link);
                }
                on:mouseout=move |e: web_sys::MouseEvent| {
                    if !preview.get_untracked() {
                        return;
                    }
                    let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
                    if el.closest(".note-preview-card, .note-transclusion, .note-references").ok().flatten().is_some() {
                        return;
                    }
                    let Some(link) = el.closest("a.note-link:not(.is-unresolved)").ok().flatten() else { return };
                    if let Some(rel) = e.related_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                        && link.contains(Some(&rel))
                    {
                        return;
                    }
                    hover_open_seq.try_update(|n| *n += 1);
                    schedule_hover_close.try_run(());
                }
                on:focusin=move |e: web_sys::FocusEvent| {
                    if !preview.get_untracked() {
                        return;
                    }
                    let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
                    if el.closest(".note-preview-card, .note-transclusion, .note-references").ok().flatten().is_some() {
                        return;
                    }
                    let Some(link) = el.closest("a.note-link:not(.is-unresolved)").ok().flatten() else { return };
                    schedule_hover_open_focus(link);
                }
                on:focusout=move |e: web_sys::FocusEvent| {
                    if !preview.get_untracked() {
                        return;
                    }
                    let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
                    if el.closest(".note-preview-card, .note-transclusion, .note-references").ok().flatten().is_some() {
                        return;
                    }
                    let Some(link) = el.closest("a.note-link:not(.is-unresolved)").ok().flatten() else { return };
                    if let Some(rel) = e.related_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                        && link.contains(Some(&rel))
                    {
                        return;
                    }
                    hover_open_seq.try_update(|n| *n += 1);
                    schedule_hover_close.try_run(());
                }>
                <h1 class="note-title">{move || {
                    let n = entry.with(|e| e.name.clone());
                    if has_md_ext(&n) { strip_md_ext(&n) } else { n }
                }}</h1>
                <div class="note-meta"><span>"Edited "{move || edited(modified.get())}</span></div>
                {move || (markdown.get() && !preview.get()).then(|| view! {
                    <MarkdownToolbar on_format=format disabled=Signal::derive(move || read_only.get() || !ready.get())>
                        <button type="button" class="text-button starts-group" aria-label="Undo" title="Undo" disabled=move || history.with(Vec::is_empty) || read_only.get()
                            on:click=move |_| {
                                if let Some(previous) = history.try_update(|h| h.pop()).flatten() {
                                    future.update(|f| f.push(text.get_untracked()));
                                    text.set(previous);
                                }
                            }><Icon name="undo"/></button>
                        <button type="button" class="text-button" aria-label="Redo" title="Redo" disabled=move || future.with(Vec::is_empty) || read_only.get()
                            on:click=move |_| {
                                if let Some(next) = future.try_update(|f| f.pop()).flatten() {
                                    history.update(|h| h.push(text.get_untracked()));
                                    text.set(next);
                                }
                            }><Icon name="redo"/></button>
                    </MarkdownToolbar>
                })}
                {move || if preview.get() {
                    view! { <Markdown source=text.get() links=preview_links/> }.into_any()
                } else {
                    let listbox = suggest_id.clone();
                    view! {
                        <textarea node_ref=input class="note-source" aria-label="File contents" spellcheck=move || markdown.get().to_string() placeholder=move || ready.get().then_some("Start writing\u{2026}")
                            readonly=move || read_only.get() || !ready.get() || recovery.with(Option::is_some)
                            aria-autocomplete="list" aria-controls=listbox.clone() aria-expanded=move || (!choices.with(Vec::is_empty)).to_string()
                            aria-activedescendant=move || (!choices.with(Vec::is_empty)).then(|| format!("{listbox}-{}", active.get()))
                            prop:value=move || text.get()
                            on:keydown=suggest_keys
                            on:paste=move |e: web_sys::ClipboardEvent| {
                                let files = file_list(e.clipboard_data().and_then(|d| d.files()));
                                if !files.is_empty() && markdown.get_untracked() {
                                    e.prevent_default();
                                    attach_files(files);
                                }
                            }
                            on:dragover=move |e: web_sys::DragEvent| {
                                if e.data_transfer().is_some_and(|d| d.types().includes(&"Files".into(), 0)) && markdown.get_untracked() {
                                    e.prevent_default();
                                }
                            }
                            on:drop=move |e: web_sys::DragEvent| {
                                let files = file_list(e.data_transfer().and_then(|d| d.files()));
                                if !files.is_empty() && markdown.get_untracked() {
                                    e.prevent_default();
                                    attach_files(files);
                                }
                            }
                            on:keyup=move |e| if matches!(e.key().as_str(), "ArrowLeft" | "ArrowRight" | "Home" | "End") { update_suggestions() }
                            on:click=move |_| update_suggestions()
                            on:blur=move |_| choices.set(Vec::new())
                            on:input=move |e| { change(event_target_value(&e)); if markdown.get_untracked() { update_suggestions(); } }></textarea>
                    }.into_any()
                }}
                <LinkSuggestions id=listbox_id.clone() items=choices active=active point=point on_pick=pick/>
                {children()}
            </div>
            <note_links::NoteLinkPreview target=hover_preview links=links on_enter=cancel_hover_close on_leave=schedule_hover_close on_close=close_hover_now/>
        </section>
    }
}
