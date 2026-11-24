//! PDF highlights kept in Notes: one note per book, `Highlights/<book title>.md` in the Notes
//! folder, which the reader appends to and reads back to draw the highlights again.
//!
//! Each highlight is a block:
//!
//! ```text
//! > The quoted text,
//! > line by line.
//! > — [Book title, p. 3](/books?book=<file id>&page=3) ^hl-k3x9q2
//!
//! [hl-k3x9q2]: /books?book=<file id>&page=3 "dar-highlight 3:0.12000,0.20000,0.50000,0.03000"
//! ```
//!
//! The link opens the book at that page. The link reference definition after it holds the
//! boxes (`page:x,y,width,height` as fractions of the page, pages from 1); Markdown shows no
//! unused definition, so the note reads as quotes in Notes and other editors alike. Writes go
//! through the Files upload API with the note's revision, so an edit elsewhere is never lost.

use serde::Deserialize;

use crate::api::{self, BrowserSession, Req};
use crate::books::pdf_text::{PdfHighlight, PdfRect, PdfSelection};
use crate::files::editor::{TextFile, write_text_file};
use crate::live::{reducer, replied};
use crate::module_bindings::create_folder_reducer::create_folder as _;

/// The Notes folder (under the Notes root) that holds highlight notes.
pub const FOLDER: &str = "Highlights";
const MARKER: &str = "dar-highlight";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotesFolder {
    id: String,
    parent_id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotesFile {
    id: String,
    parent_id: String,
    name: String,
    revision: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotesIndex {
    root_id: String,
    #[serde(default)]
    folders: Vec<NotesFolder>,
    #[serde(default)]
    notes: Vec<NotesFile>,
}

/// The note's file name for a book title: no path separators or control characters.
pub fn note_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut name = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    name = name
        .trim_matches('.')
        .trim()
        .chars()
        .take(120)
        .collect::<String>()
        .trim()
        .to_owned();
    if name.is_empty() {
        name = "Untitled book".into();
    }
    format!("{name}.md")
}

/// `text` safe inside a Markdown quote line.
fn escape_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '|' | '~' | '^' | '$'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    // A line that would start a list item stays a line of text.
    if let Some(first) = out.chars().next()
        && (matches!(first, '-' | '+')
            || (first.is_ascii_digit()
                && out
                    .trim_start_matches(|c: char| c.is_ascii_digit())
                    .starts_with(['.', ')'])))
    {
        out.insert(0, '\\');
    }
    out
}

/// A short block id for Obsidian-style `^id` references.
pub fn new_id() -> String {
    let raw = crate::random_uuid().replace('-', "");
    format!("hl-{}", &raw[..8])
}

/// The page link a highlight points to (pages from 1).
pub fn page_link(book_id: &str, page: usize) -> String {
    format!("/books?book={book_id}&page={page}")
}

/// The Markdown block for one highlight.
pub fn block(id: &str, book_id: &str, title: &str, selection: &PdfSelection) -> String {
    let page = selection.page + 1;
    let link = page_link(book_id, page);
    let mut out = String::new();
    for line in selection.text.lines().filter(|l| !l.trim().is_empty()) {
        out.push_str("> ");
        out.push_str(&escape_inline(line.trim()));
        out.push('\n');
    }
    let label = title
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    out.push_str(&format!(
        "> \u{2014} [{label}, p. {page}]({link}) ^{id}\n\n"
    ));
    let boxes = selection
        .rects
        .iter()
        .map(|r| {
            format!(
                "{}:{:.5},{:.5},{:.5},{:.5}",
                r.page + 1,
                r.x,
                r.y,
                r.width,
                r.height
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    out.push_str(&format!("[{id}]: {link} \"{MARKER} {boxes}\"\n"));
    out
}

/// The highlights of book `book_id` in a note's text.
pub fn parse(note: &str, book_id: &str) -> Vec<PdfHighlight> {
    let mut found = Vec::new();
    for line in note.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some((id, rest)) = rest.split_once("]:") else {
            continue;
        };
        if !id.starts_with("hl-") {
            continue;
        }
        let mut parts = rest.trim().splitn(2, ' ');
        let url = parts.next().unwrap_or_default();
        let title = parts.next().unwrap_or_default().trim();
        let query = url.split_once('?').map_or("", |(_, q)| q);
        if !query
            .split('&')
            .any(|pair| pair == format!("book={book_id}"))
        {
            continue;
        }
        let Some(boxes) = title
            .strip_prefix('"')
            .and_then(|t| t.strip_suffix('"'))
            .and_then(|t| t.strip_prefix(MARKER))
        else {
            continue;
        };
        let rects: Vec<PdfRect> = boxes
            .split_whitespace()
            .filter_map(|token| {
                let (page, numbers) = token.split_once(':')?;
                let page = page.parse::<usize>().ok()?.checked_sub(1)?;
                let n: Vec<f64> = numbers
                    .split(',')
                    .filter_map(|v| v.parse::<f64>().ok())
                    .collect();
                let [x, y, width, height] = n[..] else {
                    return None;
                };
                let valid = [x, y, width, height]
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v));
                valid.then_some(PdfRect {
                    page,
                    x,
                    y,
                    width,
                    height,
                })
            })
            .collect();
        if !rects.is_empty() {
            found.push(PdfHighlight {
                id: id.to_owned(),
                rects,
            });
        }
    }
    found
}

async fn notes_index() -> Result<NotesIndex, String> {
    api::get::<NotesIndex>("/api/v1/notes/library")
        .await
        .map_err(|e| e.message)
}

async fn read_text(id: &str) -> Result<String, String> {
    let response = api::fetch(&format!("/api/v1/files/{id}/content"), Req::default())
        .await
        .map_err(|e| e.message)?;
    if !response.ok() {
        return Err("The highlights note could not be read.".into());
    }
    let promise = response
        .text()
        .map_err(|_| "The highlights note could not be read.".to_owned())?;
    let text = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|_| "The highlights note could not be read.".to_owned())?;
    Ok(text.as_string().unwrap_or_default())
}

fn find_note<'a>(
    index: &'a NotesIndex,
    title: &str,
) -> (Option<&'a NotesFolder>, Option<&'a NotesFile>) {
    let folder = index
        .folders
        .iter()
        .find(|f| f.parent_id == index.root_id && f.name.eq_ignore_ascii_case(FOLDER));
    let name = note_name(title).to_lowercase();
    let note = folder.and_then(|f| {
        index
            .notes
            .iter()
            .find(|n| n.parent_id == f.id && n.name.to_lowercase() == name)
    });
    (folder, note)
}

/// The saved highlights of a book, from its note (none when there is no note yet).
pub async fn load(book_id: &str, title: &str) -> Vec<PdfHighlight> {
    let Ok(index) = notes_index().await else {
        return Vec::new();
    };
    let (_, note) = find_note(&index, title);
    let Some(note) = note else { return Vec::new() };
    match read_text(&note.id).await {
        Ok(text) => parse(&text, book_id),
        Err(_) => Vec::new(),
    }
}

/// Appends a highlight to the book's note, creating the folder and note on first use; retries
/// when the note changed in between. Returns the saved highlight.
pub async fn save(
    session: &BrowserSession,
    book_id: &str,
    title: &str,
    selection: &PdfSelection,
) -> Result<PdfHighlight, String> {
    let id = new_id();
    let addition = block(&id, book_id, title, selection);
    let name = note_name(title);
    let mut last_error = String::new();
    for _ in 0..3 {
        let index = notes_index().await?;
        let (folder, note) = find_note(&index, title);
        let folder_id = match folder {
            Some(f) => f.id.clone(),
            None => {
                let conn = crate::live::connection()
                    .ok_or_else(|| "Dar is reconnecting. Try again in a moment.".to_owned())?;
                let folder_id = crate::random_uuid();
                let (fid, parent) = (folder_id.clone(), index.root_id.clone());
                if let Err(e) = reducer(|tx| {
                    conn.reducers
                        .create_folder_then(fid, parent, FOLDER.into(), replied(tx))
                })
                .await
                {
                    last_error = e;
                    continue;
                }
                folder_id
            }
        };
        let (content, file_id, revision) = match note {
            Some(n) => {
                let current = read_text(&n.id).await?;
                let mut content = current.trim_end().to_owned();
                content.push_str("\n\n");
                content.push_str(&addition);
                (content, Some(n.id.clone()), Some(n.revision))
            }
            None => (
                format!("# Highlights: {}\n\n{addition}", title.trim()),
                None,
                None,
            ),
        };
        let file = TextFile {
            name: &name,
            parent_id: &folder_id,
            content: &content,
            media_type: "text/markdown",
            file_id: file_id.as_deref(),
            expected_revision: revision,
        };
        match write_text_file(session, file).await {
            Ok(_) => {
                return Ok(PdfHighlight {
                    id,
                    rects: selection.rects.clone(),
                });
            }
            Err(e) if matches!(e.status, 409 | 412) => last_error = e.message,
            Err(e) => return Err(e.message),
        }
    }
    Err(if last_error.is_empty() {
        "The highlight could not be saved. Try again.".into()
    } else {
        last_error
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_round_trip_and_stay_per_book() {
        let selection = PdfSelection {
            text: "First *line*\n- second [line]".into(),
            page: 2,
            rects: vec![
                PdfRect {
                    page: 2,
                    x: 0.1,
                    y: 0.2,
                    width: 0.5,
                    height: 0.03,
                },
                PdfRect {
                    page: 3,
                    x: 0.1,
                    y: 0.05,
                    width: 0.25,
                    height: 0.03,
                },
            ],
        };
        let text = block("hl-abc12345", "book-1", "A [Book]", &selection);
        assert!(
            text.starts_with("> First \\*line\\*\n> \\- second \\[line\\]\n"),
            "{text}"
        );
        assert!(text.contains("> \u{2014} [A \\[Book\\], p. 3](/books?book=book-1&page=3) ^hl-abc12345\n\n[hl-abc12345]: /books?book=book-1&page=3 \"dar-highlight 3:"), "{text}");
        let note = format!(
            "# Highlights: A [Book]\n\n{text}\n{}",
            block("hl-other000", "book-2", "Other", &selection)
        );
        let parsed = parse(&note, "book-1");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "hl-abc12345");
        assert_eq!(parsed[0].rects, selection.rects);
        assert_eq!(parse(&note, "book-2").len(), 1);
        assert!(parse(&note, "book-").is_empty());
    }

    #[test]
    fn note_names_are_safe() {
        assert_eq!(note_name("War / Peace: a novel?"), "War Peace a novel.md");
        assert_eq!(note_name("  ..  "), "Untitled book.md");
        assert_eq!(note_name("Plain"), "Plain.md");
    }
}
