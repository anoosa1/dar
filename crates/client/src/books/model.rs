//! Books domain models, filtering/sorting/series grouping, URL state, and upload helpers.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use js_sys::{Function, Promise, Reflect};
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::books::epub::ReaderLook;
use crate::shared::Preferences;
use crate::ui::{document, window};

pub const API: &str = "/api/v1/books";
pub const DEFAULT_BOOKS_FOLDER: &str = "docs/books";
pub const ACCEPT: &str = ".epub,.pdf,.cbz,.cbr,.cb7,.cbt";

pub const VIEWS: &[(&str, &str, &str)] = &[
    ("home", "Home", "library"),
    ("reading", "Reading", "reading"),
    ("want", "Want to read", "bookmark"),
    ("all", "All books", "book"),
    ("series", "Series", "stack"),
    ("finished", "Finished", "check"),
    ("highlights", "Highlights", "quote"),
];

pub const SORTS: &[(&str, &str)] = &[
    ("recent", "Recently read"),
    ("title", "Title"),
    ("author", "Author"),
    ("added", "Recently added"),
];

pub const FONT_SIZES: &[u32] = &[14, 15, 16, 17, 18, 19, 20, 21, 22, 24, 26, 28];

#[wasm_bindgen(
    inline_js = "export function language_display_name(code) { try { return new Intl.DisplayNames([navigator.language], { type: 'language' }).of(code) ?? code; } catch { return code; } }"
)]
extern "C" {
    pub fn language_display_name(code: &str) -> String;
}

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

pub fn media_type(format: &str) -> &'static str {
    match format {
        "epub" => "application/epub+zip",
        "pdf" => "application/pdf",
        "cbz" => "application/vnd.comicbook+zip",
        "cbr" => "application/vnd.comicbook-rar",
        "cb7" => "application/x-7z-compressed",
        "cbt" => "application/x-tar",
        _ => "application/octet-stream",
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    Home,
    Reading,
    Want,
    All,
    Series,
    Finished,
    Highlights,
    Library(String),
}

impl View {
    pub fn from_list_id(id: &str) -> Option<Self> {
        match id {
            "home" => Some(Self::Home),
            "reading" => Some(Self::Reading),
            "want" => Some(Self::Want),
            "all" => Some(Self::All),
            "series" => Some(Self::Series),
            "finished" => Some(Self::Finished),
            "highlights" => Some(Self::Highlights),
            _ => None,
        }
    }

    pub fn list_id(&self) -> Option<&'static str> {
        match self {
            Self::Home => Some("home"),
            Self::Reading => Some("reading"),
            Self::Want => Some("want"),
            Self::All => Some("all"),
            Self::Series => Some("series"),
            Self::Finished => Some("finished"),
            Self::Highlights => Some("highlights"),
            Self::Library(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogFilter {
    All,
    Epub,
    Pdf,
    Comic,
    Reading,
    Finished,
    Author(String),
    Tag(String),
    SeriesName(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeriesGroup {
    pub name: String,
    pub authors: String,
    pub books: Vec<BookItem>,
    pub completed: usize,
    pub progress: f64,
    pub status_summary: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BookTransfer {
    pub id: String,
    pub name: String,
    pub progress: f64,
    pub status: &'static str,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Where {
    pub position: BookPosition,
    pub progress: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub state: BookState,
    pub position: Option<BookPosition>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadReservation {
    pub id: String,
}

pub fn percent(value: f64) -> String {
    format!("{}%", (value.clamp(0.0, 1.0) * 100.0).round() as i64)
}

pub fn authors(book: &BookItem) -> String {
    if !book.authors.is_empty() {
        book.authors.join(", ")
    } else if book.format == "pdf" {
        "PDF".into()
    } else if is_comic(&book.format) {
        "Comic".into()
    } else {
        "Unknown author".into()
    }
}

/// Rough reading time remaining for text books (EPUB/PDF with page count).
pub fn time_left(book: &BookItem, progress: f64) -> Option<String> {
    if book.pages == 0 || is_comic(&book.format) || progress >= 0.995 {
        return None;
    }
    let remaining_pages = ((book.pages as f64) * (1.0 - progress.clamp(0.0, 1.0))).max(1.0);
    // ~1.4 minutes per estimated page for long-form prose
    let mins = (remaining_pages * 1.4).round() as usize;
    if mins < 1 {
        Some("<1 min left".into())
    } else if mins < 60 {
        Some(format!("~{mins} min left"))
    } else {
        let h = mins / 60;
        let m = mins % 60;
        if m == 0 {
            Some(format!("~{h}h left"))
        } else {
            Some(format!("~{h}h {m}m left"))
        }
    }
}

/// A short name for this browser, shown as where a book was last read.
pub fn device_name() -> String {
    let ua = window().navigator().user_agent().unwrap_or_default();
    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Chrome/") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "Browser"
    };
    let os = if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Mac OS X") {
        "macOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        ""
    };
    if os.is_empty() {
        browser.into()
    } else {
        format!("{browser} on {os}")
    }
}

/// The view, the selected book, whether it is open and the page to open it at (`page`, from
/// 1, as highlight links in Notes carry it; a page opens the reader).
pub fn read_view() -> (View, Option<String>, bool, Option<usize>) {
    let search = window().location().search().unwrap_or_default();
    let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) else {
        return (View::Home, None, false, None);
    };
    let view = if let Some(lib) = params.get("library").filter(|s| !s.is_empty()) {
        View::Library(lib)
    } else {
        params
            .get("view")
            .as_deref()
            .and_then(View::from_list_id)
            .unwrap_or(View::Home)
    };
    let book = params.get("book").filter(|s| !s.is_empty());
    let page = params
        .get("page")
        .and_then(|p| p.parse::<usize>().ok())
        .and_then(|p| p.checked_sub(1))
        .filter(|_| book.is_some());
    let read = params.get("read").as_deref() == Some("1") || page.is_some();
    (view, book, read, page)
}

pub fn status_label(status: &str) -> &'static str {
    match status {
        "want" => "Want to read",
        "reading" => "Reading",
        "finished" => "Finished",
        _ => "",
    }
}

/// Covers with no image get a typographic cover in one of a fixed set of hues.
pub fn hue(text: &str) -> u32 {
    let mut h: u32 = 0;
    for c in text.chars() {
        h = (h * 31 + (c as u32)) % 9973;
    }
    h % 10
}

pub fn place(book: &BookItem) -> String {
    let Some(state) = &book.state else {
        return "Not started".into();
    };
    let Some(p) = &state.position else {
        return "Not started".into();
    };
    if let Some(page) = p.page {
        if book.pages > 0 {
            format!("Page {} of {}", page + 1, book.pages)
        } else {
            format!("Page {}", page + 1)
        }
    } else {
        format!("{} read", percent(state.progress))
    }
}

/// Clean progress caption for Details without repeating the percentage twice.
pub fn progress_caption(book: &BookItem) -> String {
    let Some(state) = &book.state else {
        return String::new();
    };
    let pct = percent(state.progress);
    if let Some(p) = &state.position
        && let Some(page) = p.page
    {
        return if book.pages > 0 {
            format!("{pct} \u{00b7} Page {} of {}", page + 1, book.pages)
        } else {
            format!("{pct} \u{00b7} Page {}", page + 1)
        };
    }
    match time_left(book, state.progress) {
        Some(est) => format!("{pct} read \u{00b7} {est}"),
        None => format!("{pct} read"),
    }
}

pub fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

pub fn pref_str<'a>(p: &'a Preferences, key: &str) -> Option<&'a str> {
    p.rest.get(key).and_then(|v| v.as_str())
}

pub fn pref_u32(p: &Preferences, key: &str) -> Option<u32> {
    p.rest.get(key).and_then(|v| v.as_u64()).map(|n| n as u32)
}

pub fn sort_books(list: &[BookItem], sort: &str) -> Vec<BookItem> {
    let mut out = list.to_vec();
    out.sort_by(|a, b| match sort {
        "title" => locale_cmp(&a.title, &b.title),
        "author" => {
            locale_cmp(&authors(a), &authors(b)).then_with(|| locale_cmp(&a.title, &b.title))
        }
        "added" => b.modified_at.total_cmp(&a.modified_at),
        _ => {
            let au = a.state.as_ref().map_or(0.0, |s| s.updated_at);
            let bu = b.state.as_ref().map_or(0.0, |s| s.updated_at);
            bu.total_cmp(&au)
                .then_with(|| b.modified_at.total_cmp(&a.modified_at))
        }
    });
    out
}

pub fn filter_books(list: &[BookItem], filter: &CatalogFilter) -> Vec<BookItem> {
    list.iter()
        .filter(|b| match filter {
            CatalogFilter::All => true,
            CatalogFilter::Epub => b.format == "epub",
            CatalogFilter::Pdf => b.format == "pdf",
            CatalogFilter::Comic => is_comic(&b.format),
            CatalogFilter::Reading => b.state.as_ref().is_some_and(|s| s.status == "reading"),
            CatalogFilter::Finished => b.state.as_ref().is_some_and(|s| s.status == "finished"),
            CatalogFilter::Author(name) => b.authors.iter().any(|a| a.eq_ignore_ascii_case(name)),
            CatalogFilter::Tag(tag) => b.subjects.iter().any(|s| s.eq_ignore_ascii_case(tag)),
            CatalogFilter::SeriesName(series) => b.series.eq_ignore_ascii_case(series),
        })
        .cloned()
        .collect()
}

pub fn group_series(books: &[BookItem]) -> Vec<SeriesGroup> {
    let mut map: BTreeMap<String, Vec<BookItem>> = BTreeMap::new();
    for b in books {
        if !b.series.trim().is_empty() {
            map.entry(b.series.trim().to_owned())
                .or_default()
                .push(b.clone());
        }
    }
    let mut groups: Vec<SeriesGroup> = map
        .into_iter()
        .map(|(name, mut items)| {
            items.sort_by(|a, b| {
                a.series_index
                    .unwrap_or(1e9)
                    .total_cmp(&b.series_index.unwrap_or(1e9))
                    .then_with(|| locale_cmp(&a.title, &b.title))
            });
            let completed = items
                .iter()
                .filter(|b| b.state.as_ref().is_some_and(|s| s.status == "finished"))
                .count();
            let total_prog: f64 = items
                .iter()
                .map(|b| match b.state.as_ref() {
                    Some(s) if s.status == "finished" => 1.0,
                    Some(s) => s.progress.clamp(0.0, 1.0),
                    None => 0.0,
                })
                .sum();
            let progress = if items.is_empty() {
                0.0
            } else {
                total_prog / items.len() as f64
            };
            let first_author = items
                .first()
                .map(authors)
                .unwrap_or_else(|| "Unknown author".into());
            let status_summary = if completed == items.len() {
                "Completed".into()
            } else if let Some(active) = items
                .iter()
                .find(|b| b.state.as_ref().is_some_and(|s| s.status == "reading"))
            {
                let p = active.state.as_ref().map_or(0.0, |s| s.progress);
                match active.series_index {
                    Some(idx) => format!("Book {idx} in progress ({})", percent(p)),
                    None => format!("In progress ({})", percent(p)),
                }
            } else if completed > 0 {
                format!("{completed} of {} finished", items.len())
            } else {
                "Not started".into()
            };
            SeriesGroup {
                name,
                authors: first_author,
                books: items,
                completed,
                progress,
                status_summary,
            }
        })
        .collect();
    groups.sort_by(|a, b| locale_cmp(&a.name, &b.name));
    groups
}

pub fn line_height_value(name: &str) -> f64 {
    match name {
        "tight" => 1.35,
        "loose" => 1.9,
        _ => 1.6,
    }
}

pub fn fixed_theme(theme: &str) -> (&'static str, &'static str, &'static str, bool) {
    match theme {
        "sepia" => ("#f4ecd8", "#4b3a2c", "#8a4b23", false),
        "night" => ("#141112", "#d9d0c5", "#e8a0a8", true),
        _ => ("#ffffff", "#1f1a17", "#7a3b6f", false),
    }
}

pub fn default_look() -> ReaderLook {
    let (bg, color, link, dark) = fixed_theme("paper");
    ReaderLook {
        background: bg.into(),
        color: color.into(),
        link: link.into(),
        dark,
        font: "serif".into(),
        size: 18,
        line_height: 1.6,
        layout: "pages".into(),
        columns: "auto".into(),
    }
}

/// A Files folder path like "docs/books": 1-8 names, no dots-only or control characters.
pub fn books_folder_parts(value: &str) -> Option<Vec<String>> {
    if value.encode_utf16().count() > 500 {
        return None;
    }
    let parts: Vec<String> = value
        .split('/')
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty()
        || parts.len() > 8
        || parts.iter().any(|part| {
            part.encode_utf16().count() > 255
                || part == "."
                || part == ".."
                || part.chars().any(|c| (c as u32) <= 0x1f || c == '\\')
        })
    {
        return None;
    }
    Some(parts)
}

pub fn dedupe_filename(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(&name.to_lowercase()) {
        return name.to_owned();
    }
    for n in 2..1000 {
        let candidate = match name.rfind('.') {
            Some(dot) => format!("{} {n}{}", &name[..dot], &name[dot..]),
            None => format!("{name} {n}"),
        };
        if !taken.contains(&candidate.to_lowercase()) {
            return candidate;
        }
    }
    name.to_owned()
}

pub fn is_fullscreen() -> bool {
    Reflect::get(&document(), &JsValue::from_str("fullscreenElement"))
        .ok()
        .is_some_and(|v| !v.is_null() && !v.is_undefined())
}

pub fn call_promise_method(target: &JsValue, name: &str) {
    if let Ok(val) = Reflect::get(target, &JsValue::from_str(name))
        && let Some(func) = val.dyn_ref::<Function>()
        && let Ok(ret) = func.call0(target)
        && let Ok(promise) = ret.dyn_into::<Promise>()
    {
        spawn_local(async move {
            let _ = JsFuture::from(promise).await;
        });
    }
}

pub async fn upload_blob_xhr(
    url: &str,
    csrf: &str,
    file: &web_sys::File,
    on_progress: impl Fn(f64) + 'static,
) -> Result<(), String> {
    let xhr = web_sys::XmlHttpRequest::new().map_err(|_| "The upload failed.".to_owned())?;
    let (sender, receiver) = futures::channel::oneshot::channel::<Result<(), String>>();
    let sender = Rc::new(RefCell::new(Some(sender)));
    let finish = {
        let sender = sender.clone();
        move |res: Result<(), String>| {
            if let Some(s) = sender.borrow_mut().take() {
                let _ = s.send(res);
            }
        }
    };
    let _ = xhr.open("PUT", url);
    let _ = xhr.set_request_header("x-csrf-token", csrf);
    let progress_cb =
        Closure::<dyn FnMut(web_sys::ProgressEvent)>::new(move |e: web_sys::ProgressEvent| {
            let ratio = if e.total() > 0.0 {
                e.loaded() / e.total()
            } else {
                0.0
            };
            on_progress(ratio);
        });
    if let Ok(upload) = xhr.upload() {
        upload.set_onprogress(Some(progress_cb.as_ref().unchecked_ref()));
    }
    let load_cb = {
        let (xhr, finish) = (xhr.clone(), finish.clone());
        Closure::<dyn FnMut()>::new(move || {
            let status = xhr.status().unwrap_or(0);
            if status < 300 {
                finish(Ok(()));
            } else {
                let text = xhr.response_text().ok().flatten().unwrap_or_default();
                let msg = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "The upload failed.".into());
                finish(Err(msg));
            }
        })
    };
    let error_cb = {
        let finish = finish.clone();
        Closure::<dyn FnMut()>::new(move || {
            finish(Err("The connection was lost during the upload.".into()));
        })
    };
    xhr.set_onload(Some(load_cb.as_ref().unchecked_ref()));
    xhr.set_onerror(Some(error_cb.as_ref().unchecked_ref()));
    let _ = xhr.send_with_opt_blob(Some(file));
    let result = receiver
        .await
        .unwrap_or_else(|_| Err("The upload failed.".into()));
    drop((progress_cb, load_cb, error_cb));
    result
}
