//! Books (`next/client/books/`): a library over a Files folder with EPUB, PDF and comic readers.

pub mod comic;
pub mod epub;
pub mod highlights;
pub mod pdf;
pub mod pdf_text;
pub mod workspace;

pub use pdf::PdfView;
pub use workspace::BooksWorkspace;

use serde::{Deserialize, Serialize};

pub const DEFAULT_BOOKS_FOLDER: &str = "docs/books";

/// The book format from a file name; hidden files and other types are not books.
pub fn book_format(name: &str) -> Option<&'static str> {
    if name.starts_with('.') {
        return None;
    }
    let ext = name.rsplit_once('.')?.1;
    if ext.is_empty() || ext.len() > 8 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    match ext.to_ascii_lowercase().as_str() {
        "epub" => Some("epub"),
        "pdf" => Some("pdf"),
        "cbz" => Some("cbz"),
        "cbr" => Some("cbr"),
        "cb7" => Some("cb7"),
        "cbt" => Some("cbt"),
        _ => None,
    }
}

pub fn is_comic(format: &str) -> bool {
    format != "epub" && format != "pdf"
}

pub fn book_format_label(format: &str) -> String {
    format.to_ascii_uppercase()
}

/// Where a reader stopped. EPUB: spine item and fraction through it; PDF and comics:
/// page (from 0) and fraction down that page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spine: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(default)]
    pub fraction: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookState {
    pub file_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub position: Option<BookPosition>,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub updated_at: f64,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookMark {
    pub id: String,
    pub file_id: String,
    pub position: BookPosition,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub created_at: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookItem {
    pub id: String,
    pub parent_id: String,
    pub name: String,
    pub format: String,
    #[serde(default)]
    pub size: f64,
    #[serde(default)]
    pub revision: u32,
    #[serde(default)]
    pub modified_at: f64,
    #[serde(default)]
    pub library: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub state: Option<BookState>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub series: String,
    #[serde(default)]
    pub series_index: Option<f64>,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub published: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub subjects: Vec<String>,
    #[serde(default)]
    pub pages: usize,
    #[serde(default)]
    pub cover: bool,
    #[serde(default)]
    pub right_to_left: bool,
    #[serde(default)]
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookLibraryFolder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookLibrary {
    pub root_id: String,
    pub path: String,
    #[serde(default)]
    pub libraries: Vec<BookLibraryFolder>,
    #[serde(default)]
    pub books: Vec<BookItem>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub indexing: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookTocEntry {
    pub label: String,
    pub href: String,
    #[serde(default)]
    pub depth: usize,
    #[serde(default)]
    pub spine: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpineItem {
    pub href: String,
    #[serde(default)]
    pub size: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BookManifest {
    #[serde(rename_all = "camelCase")]
    Epub {
        #[serde(default)]
        spine: Vec<SpineItem>,
        #[serde(default)]
        toc: Vec<BookTocEntry>,
        #[serde(default)]
        right_to_left: bool,
        #[serde(default)]
        language: String,
    },
    #[serde(rename_all = "camelCase")]
    Comic {
        #[serde(default)]
        pages: usize,
        #[serde(default)]
        right_to_left: bool,
    },
    Pdf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookDetail {
    #[serde(default)]
    pub state: Option<BookState>,
    #[serde(default)]
    pub marks: Vec<BookMark>,
    pub manifest: BookManifest,
}
