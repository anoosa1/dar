//! Notes settings › Import a Logseq graph: reads the picked graph's Markdown files (several files,
//! or the graph folder with `webkitRelativePath`), converts them with `dar_notes::logseq` and
//! writes them into the Notes folder through the Files API. Folders are made as needed and
//! nothing is overwritten: a taken name gets ` 2`.

use std::collections::HashMap;

use dar_notes::logseq::{self, Source};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::note_links::NoteFiles;
use super::notes::NotesLibrary;
use crate::api::{self, BrowserSession};
use crate::files::editor::{TextFile, write_text_file};
use crate::live::{Connection, Live, reducer, replied};
use crate::module_bindings::create_folder;
use crate::pim::PimLive;

/// Files Notes can edit (`write_text_file`'s limit).
const MAX_BYTES: f64 = 1024.0 * 1024.0;
const MAX_FILES: usize = 5000;

#[derive(Clone, Debug, Default, PartialEq)]
struct Summary {
    imported: usize,
    folders: usize,
    references: usize,
    unresolved: usize,
    skipped: usize,
    failed: Vec<String>,
}

impl Summary {
    fn lines(&self) -> Vec<String> {
        let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
        let mut lines = vec![format!("Imported {} into Notes.", plural(self.imported, "note", "notes"))];
        if self.folders > 0 {
            lines.push(format!("Made {}.", plural(self.folders, "folder", "folders")));
        }
        if self.references > 0 {
            lines.push(format!("Linked {}.", plural(self.references, "block reference", "block references")));
        }
        match self.unresolved {
            0 => {}
            1 => lines.push("1 block reference names a block that was not imported; it stays as written.".into()),
            n => lines.push(format!("{n} block references name blocks that were not imported; they stay as written.")),
        }
        match self.skipped {
            0 => {}
            1 => lines.push("Skipped 1 file that is not a page or journal.".into()),
            n => lines.push(format!("Skipped {n} files that are not pages or journals.")),
        }
        lines
    }
}

fn picked(input: NodeRef<html::Input>) -> Vec<web_sys::File> {
    input.get_untracked().and_then(|i| i.files()).map(|list| (0..list.length()).filter_map(|i| list.item(i)).collect()).unwrap_or_default()
}

/// The file's path inside the picked folder, or its name for single files.
fn relative_path(file: &web_sys::File) -> String {
    js_sys::Reflect::get(file, &"webkitRelativePath".into()).ok().and_then(|v| v.as_string()).filter(|p| !p.is_empty()).unwrap_or_else(|| file.name())
}

/// The folder at `dir` (relative to the Notes folder), made with its parents when missing.
async fn folder_id(conn: &Connection, root: &str, dir: &str, folders: &mut HashMap<String, String>, made: &mut usize) -> Result<String, String> {
    let mut parent = root.to_owned();
    let mut path = String::new();
    for part in dir.split('/').filter(|p| !p.is_empty()) {
        path = if path.is_empty() { part.to_owned() } else { format!("{path}/{part}") };
        if let Some(id) = folders.get(&path.to_lowercase()) {
            parent = id.clone();
            continue;
        }
        let id = crate::random_uuid();
        let (folder, under, name) = (id.clone(), parent.clone(), part.to_owned());
        reducer(|tx| conn.reducers.create_folder_then(folder, under, name, replied(tx))).await.map_err(|_| format!("the folder {path} could not be made"))?;
        folders.insert(path.to_lowercase(), id.clone());
        *made += 1;
        parent = id;
    }
    Ok(parent)
}

async fn import(live: Live, session: &BrowserSession, files: Vec<web_sys::File>, progress: RwSignal<String>) -> Result<Summary, String> {
    let conn = live.connection.get_untracked().ok_or("Notes are reconnecting. Try again in a moment.")?;
    if files.len() > MAX_FILES {
        return Err(format!("A Logseq import takes at most {MAX_FILES} files."));
    }
    let library: NotesLibrary = api::get("/api/v1/notes/library").await.map_err(|e| e.message)?;
    if library.root_id.is_empty() {
        return Err("The Notes folder is not available.".into());
    }
    let others = api::get::<NoteFiles>("/api/v1/notes/attachments").await.map(|f| f.files).unwrap_or_default();
    let mut summary = Summary::default();
    let mut sources: Vec<Source> = Vec::new();
    let total = files.len();
    for (n, file) in files.into_iter().enumerate() {
        let path = relative_path(&file);
        // Files that are not pages or journals are only counted, never read.
        if logseq::note_path(&path).is_none() {
            sources.push(Source { path, text: String::new() });
            continue;
        }
        if file.size() > MAX_BYTES {
            summary.failed.push(format!("{path}: larger than 1 MiB"));
            continue;
        }
        progress.try_set(format!("Reading {} of {total}\u{2026}", n + 1));
        match wasm_bindgen_futures::JsFuture::from(file.text()).await.ok().and_then(|v| v.as_string()) {
            Some(text) => sources.push(Source { path, text }),
            None => summary.failed.push(format!("{path}: could not be read")),
        }
    }
    let existing: Vec<String> = library.notes.iter().map(|n| n.path.clone()).chain(others.iter().map(|f| f.path.clone())).collect();
    let converted = logseq::convert(&sources, &existing);
    summary.skipped = converted.skipped.len();
    summary.references = converted.references;
    summary.unresolved = converted.unresolved;
    if converted.notes.is_empty() {
        return Err("No Logseq pages or journals were found. Pick the Markdown files in the graph\u{2019}s pages and journals folders, or the graph folder.".into());
    }
    let mut folders: HashMap<String, String> = library.folders.iter().map(|f| (f.path.to_lowercase(), f.id.clone())).collect();
    let count = converted.notes.len();
    for (n, note) in converted.notes.iter().enumerate() {
        progress.try_set(format!("Importing {} of {count}\u{2026}", n + 1));
        let (dir, name) = note.path.rsplit_once('/').unwrap_or(("", note.path.as_str()));
        let parent = match folder_id(&conn, &library.root_id, dir, &mut folders, &mut summary.folders).await {
            Ok(id) => id,
            Err(e) => {
                summary.failed.push(format!("{}: {e}", note.source));
                continue;
            }
        };
        let file = TextFile { name, parent_id: &parent, content: &note.text, media_type: "text/markdown", file_id: None, expected_revision: None };
        match write_text_file(session, file).await {
            Ok(_) => summary.imported += 1,
            Err(e) => summary.failed.push(format!("{}: {}", note.source, e.message)),
        }
    }
    Ok(summary)
}

/// The Logseq part of Notes settings › Import and export.
#[component]
pub(super) fn LogseqImport(session: BrowserSession, pim: PimLive) -> impl IntoView {
    let live = expect_context::<Live>();
    let session = StoredValue::new(session);
    let files_input = NodeRef::<html::Input>::new();
    let folder_input = NodeRef::<html::Input>::new();
    // The directory picker (`webkitdirectory`, which `view!` does not know).
    Effect::new(move |_| {
        if let Some(input) = folder_input.get() {
            let _ = input.set_attribute("webkitdirectory", "");
        }
    });
    let selected = RwSignal::new(0usize);
    let busy = RwSignal::new(false);
    let progress = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let summary: RwSignal<Option<Summary>> = RwSignal::new(None);
    let count = move || selected.set(picked(files_input).len() + picked(folder_input).len());

    let start = move || {
        let files: Vec<web_sys::File> = picked(files_input).into_iter().chain(picked(folder_input)).collect();
        if files.is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        summary.set(None);
        let sess = session.get_value();
        spawn_local(async move {
            let outcome = import(live, &sess, files, progress).await;
            match outcome {
                Ok(done) => {
                    summary.try_set(Some(done));
                    for input in [files_input, folder_input] {
                        if let Some(i) = input.get_untracked() {
                            i.set_value("");
                        }
                    }
                    selected.try_set(0);
                }
                Err(e) => {
                    error.try_set(e);
                }
            }
            progress.try_set(String::new());
            busy.try_set(false);
            // The Notes list reloads.
            pim.epoch.try_update(|e| *e += 1);
        });
    };

    view! {
        <section aria-label="Logseq import">
            <h4>"Import a Logseq graph"</h4>
            <p class="field-hint">"Pages and journals become notes in the Notes folder: namespaces become folders, journals go to Journal, block references become block links and page properties become note properties. Existing notes are never replaced."</p>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            <label>"Logseq Markdown files"<input type="file" node_ref=files_input multiple accept=".md,.markdown,text/markdown" disabled=move || busy.get() on:change=move |_| count()/></label>
            <label>"Or the graph folder"<input type="file" node_ref=folder_input multiple disabled=move || busy.get() on:change=move |_| count()/></label>
            <button type="button" class="primary-button" disabled=move || busy.get() || selected.get() == 0 on:click=move |_| start()>
                {move || if busy.get() { "Importing\u{2026}".to_owned() } else { "Import Logseq graph".to_owned() }}
            </button>
            {move || (!progress.get().is_empty()).then(|| view! { <p role="status">{progress.get()}</p> })}
            {move || summary.get().map(|s| {
                let failed = s.failed.clone();
                view! {
                    <div role="status">
                        {s.lines().into_iter().map(|line| view! { <p>{line}</p> }).collect_view()}
                        {(!failed.is_empty()).then(|| view! {
                            <p>{format!("Not imported ({}):", failed.len())}</p>
                            <ul>{failed.into_iter().map(|f| view! { <li>{f}</li> }).collect_view()}</ul>
                        })}
                    </div>
                }
            })}
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::Summary;

    #[test]
    fn summary_lines() {
        let summary = Summary { imported: 1, folders: 2, references: 3, unresolved: 1, skipped: 4, failed: Vec::new() };
        assert_eq!(
            summary.lines(),
            [
                "Imported 1 note into Notes.",
                "Made 2 folders.",
                "Linked 3 block references.",
                "1 block reference names a block that was not imported; it stays as written.",
                "Skipped 4 files that are not pages or journals.",
            ]
        );
        let one = Summary { imported: 2, skipped: 1, ..Summary::default() };
        assert_eq!(one.lines(), ["Imported 2 notes into Notes.", "Skipped 1 file that is not a page or journal."]);
        assert_eq!(Summary::default().lines(), ["Imported 0 notes into Notes."]);
    }
}
