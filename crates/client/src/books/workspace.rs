//! Books workspace (`next/client/books/workspace.tsx`): a Kavita-style library over a
//! Files folder with shelves, libraries (its top folders) and a details sheet; books open
//! in the middle sheet with contents and bookmarks on the right. Reading state syncs per
//! account.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use js_sys::{Function, Promise, Reflect};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::api::{self, BrowserSession, Req, format_bytes};
use crate::books::comic::{ComicHandle, ComicView};
use crate::books::epub::{EpubHandle, EpubView, ReaderLook};
use crate::books::highlights;
use crate::books::pdf::{PdfHandle, PdfOutlineEntry, PdfReady, PdfView, PdfZoomChange};
use crate::books::pdf_text::{PdfFindState, PdfHighlight, PdfSelection};
use crate::books::{
    BookDetail, BookItem, BookLibrary, BookManifest, BookMark, BookPosition, BookState,
    BookTocEntry, DEFAULT_BOOKS_FOLDER, book_format, book_format_label, is_comic,
};
use crate::dates::edited;
use crate::icons::Icon;
use crate::live::{Live, reducer, replied};
use crate::module_bindings::trash_entry_reducer::trash_entry as _;
use crate::settings::{
    AppSettingsButton, Notice, SaveProblems, Saver, Segmented, SettingRow, SettingsSheet,
};
use crate::shared::{PREFERENCES, Preferences, context, save_preferences};
use crate::shell::WorkspaceHeader;
use crate::ui::{
    ActionMenu, Dialog, MenuAction, Select, active_element, document, listen, unique_id,
    use_floating, window,
};

const API: &str = "/api/v1/books";
const ACCEPT: &str = ".epub,.pdf,.cbz,.cbr,.cb7,.cbt";
const VIEWS: &[(&str, &str, &str)] = &[
    ("home", "Home", "library"),
    ("reading", "Reading", "reading"),
    ("want", "Want to read", "bookmark"),
    ("all", "All books", "book"),
    ("finished", "Finished", "check"),
];
const SORTS: &[(&str, &str)] = &[
    ("recent", "Recently read"),
    ("title", "Title"),
    ("author", "Author"),
    ("added", "Recently added"),
];
const FONT_SIZES: &[u32] = &[14, 15, 16, 17, 18, 19, 20, 21, 22, 24, 26, 28];

#[wasm_bindgen(
    inline_js = "export function language_display_name(code) { try { return new Intl.DisplayNames([navigator.language], { type: 'language' }).of(code) ?? code; } catch { return code; } }"
)]
extern "C" {
    fn language_display_name(code: &str) -> String;
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum View {
    Home,
    Reading,
    Want,
    All,
    Finished,
    Library(String),
}

impl View {
    fn from_list_id(id: &str) -> Option<Self> {
        match id {
            "home" => Some(Self::Home),
            "reading" => Some(Self::Reading),
            "want" => Some(Self::Want),
            "all" => Some(Self::All),
            "finished" => Some(Self::Finished),
            _ => None,
        }
    }

    fn list_id(&self) -> Option<&'static str> {
        match self {
            Self::Home => Some("home"),
            Self::Reading => Some("reading"),
            Self::Want => Some("want"),
            Self::All => Some("all"),
            Self::Finished => Some("finished"),
            Self::Library(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct BookTransfer {
    id: String,
    name: String,
    progress: f64,
    status: &'static str,
    message: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct Where {
    position: BookPosition,
    progress: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct Conflict {
    state: BookState,
    position: Option<BookPosition>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadReservation {
    id: String,
}

fn media_type(format: &str) -> &'static str {
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

fn percent(value: f64) -> String {
    format!("{}%", (value * 100.0).round() as i64)
}

fn authors(book: &BookItem) -> String {
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

/// A short name for this browser, shown as where a book was last read.
fn device_name() -> String {
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
fn read_view() -> (View, Option<String>, bool, Option<usize>) {
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

fn status_label(status: &str) -> &'static str {
    match status {
        "want" => "Want to read",
        "reading" => "Reading",
        "finished" => "Finished",
        _ => "",
    }
}

/// Covers with no image get a typographic cover in one of a fixed set of hues.
fn hue(text: &str) -> u32 {
    let mut h: u32 = 0;
    for c in text.chars() {
        h = (h * 31 + (c as u32)) % 9973;
    }
    h % 10
}

fn place(book: &BookItem) -> String {
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
        format!("{} through", percent(state.progress))
    }
}

fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

fn pref_str<'a>(p: &'a Preferences, key: &str) -> Option<&'a str> {
    p.rest.get(key).and_then(|v| v.as_str())
}

fn pref_u32(p: &Preferences, key: &str) -> Option<u32> {
    p.rest.get(key).and_then(|v| v.as_u64()).map(|n| n as u32)
}

fn sort_books(list: &[BookItem], sort: &str) -> Vec<BookItem> {
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

fn line_height_value(name: &str) -> f64 {
    match name {
        "tight" => 1.35,
        "loose" => 1.9,
        _ => 1.6,
    }
}

fn fixed_theme(theme: &str) -> (&'static str, &'static str, &'static str, bool) {
    match theme {
        "sepia" => ("#f4ecd8", "#4b3a2c", "#8a4b23", false),
        "night" => ("#141112", "#d9d0c5", "#e8a0a8", true),
        _ => ("#ffffff", "#1f1a17", "#7a3b6f", false),
    }
}

fn default_look() -> ReaderLook {
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
    }
}

/// A Files folder path like "docs/books": 1-8 names, no dots-only or control characters.
fn photo_folder_parts(value: &str) -> Option<Vec<String>> {
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

fn dedupe_filename(name: &str, taken: &HashSet<String>) -> String {
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

fn is_fullscreen() -> bool {
    Reflect::get(&document(), &JsValue::from_str("fullscreenElement"))
        .ok()
        .is_some_and(|v| !v.is_null() && !v.is_undefined())
}

fn call_promise_method(target: &JsValue, name: &str) {
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

async fn upload_blob_xhr(
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

#[component]
pub fn BooksWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let (start_view, start_book, start_read, start_page) = read_view();
    // A page from the address opens the first reader at that page, once.
    let deep_page: StoredValue<Option<(String, usize)>> =
        StoredValue::new(start_book.clone().zip(start_page));
    let library: RwSignal<Option<BookLibrary>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let view = RwSignal::new(start_view);
    let selected: RwSignal<Option<String>> = RwSignal::new(start_book.clone());
    let reading: RwSignal<Option<String>> =
        RwSignal::new(if start_read { start_book } else { None });
    let settings = RwSignal::new(false);
    let notice: RwSignal<Option<Notice>> = RwSignal::new(None);
    let reload = RwSignal::new(0u32);
    let transfers: RwSignal<Vec<BookTransfer>> = RwSignal::new(Vec::new());
    let confirm: RwSignal<Option<BookItem>> = RwSignal::new(None);

    let navigation = RwSignal::new(false);
    let navigation_id = unique_id();
    let navigation_ref = NodeRef::<html::Aside>::new();
    let nav_trigger: StoredValue<Option<SendWrapper<web_sys::HtmlElement>>> =
        StoredValue::new(None);
    let nav_was_open = StoredValue::new(false);

    let file_input = NodeRef::<html::Input>::new();
    let shared = context();
    let live = expect_context::<Live>();
    let session_sv = StoredValue::new(session);
    let pref_queue = StoredValue::new(Arc::new(futures::lock::Mutex::new(())));

    let sort = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksSort"))
                .unwrap_or("recent")
                .to_owned()
        })
    });
    let books_folder_pref = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksFolder"))
                .map(str::to_owned)
        })
    });

    // Mobile navigation focus restoration and Escape dismissal.
    Effect::new(move |_| {
        let open = navigation.get();
        if open {
            nav_trigger.set_value(active_element().map(SendWrapper::new));
            if let Some(aside) = navigation_ref.get_untracked()
                && let Ok(Some(first)) = aside.query_selector("button,a")
                && let Ok(el) = first.dyn_into::<web_sys::HtmlElement>()
            {
                let _ = el.focus();
            }
            listen(&document(), "keydown", false, move |event| {
                let ke: web_sys::KeyboardEvent = event.unchecked_into();
                if ke.key() == "Escape" && !ke.default_prevented() {
                    ke.prevent_default();
                    navigation.set(false);
                }
            });
        } else if nav_was_open.get_value() {
            let active = document().active_element();
            let in_body = active.as_ref()
                == document()
                    .body()
                    .map(JsCast::unchecked_into::<web_sys::Element>)
                    .as_ref();
            let in_nav = navigation_ref.get_untracked().is_some_and(|n| {
                n.contains(active.as_ref().map(JsCast::unchecked_ref::<web_sys::Node>))
            });
            if (in_body || in_nav)
                && let Some(prev) = nav_trigger.try_update_value(|slot| slot.take()).flatten()
            {
                let _ = prev.take().focus();
            }
        }
        nav_was_open.set_value(open);
    });

    // Load library on reload or when booksFolder changes.
    let load_abort: StoredValue<Option<SendWrapper<web_sys::AbortController>>> =
        StoredValue::new(None);
    let abort_load = move || {
        load_abort.update_value(|slot| {
            if let Some(ctrl) = slot.take() {
                ctrl.take().abort();
            }
        });
    };
    on_cleanup(move || abort_load());

    Effect::new(move |_| {
        let _ = reload.get();
        let _ = books_folder_pref.get();
        abort_load();
        let Ok(ctrl) = web_sys::AbortController::new() else {
            return;
        };
        let signal = ctrl.signal();
        load_abort.set_value(Some(SendWrapper::new(ctrl)));
        spawn_local(async move {
            match api::request::<BookLibrary>(
                &format!("{API}/library"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(val) => {
                    if !signal.aborted() {
                        let _ = library.try_set(Some(val));
                        let _ = error.try_set(String::new());
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        let _ = error.try_set(e.message);
                    }
                }
            }
        });
    });

    // New books are read in the background; poll every 3s until indexing reaches 0.
    let poll_seq = RwSignal::new(0u32);
    Effect::new(move |_| {
        let indexing = library.with(|l| l.as_ref().map_or(0, |l| l.indexing));
        if indexing == 0 {
            return;
        }
        let id = poll_seq.get_untracked() + 1;
        poll_seq.set(id);
        spawn_local(async move {
            crate::sleep(3000).await;
            if poll_seq.try_get_untracked() == Some(id) {
                let _ = reload.try_update(|n| *n += 1);
            }
        });
    });

    // Reload on window focus when not reading.
    listen(&window(), "focus", false, move |_| {
        if reading.with_untracked(Option::is_none) {
            reload.update(|n| *n += 1);
        }
    });

    // Keep URL search params synced with view / selected / reading.
    Effect::new(move |_| {
        let v = view.get();
        let sel = selected.get();
        let rd = reading.get();
        if let Ok(href) = window().location().href()
            && let Ok(url) = web_sys::Url::new(&href)
        {
            let params = url.search_params();
            for key in ["view", "library", "book", "read", "page"] {
                params.delete(key);
            }
            match &v {
                View::Library(id) => params.set("library", id),
                View::Home => {}
                other => {
                    if let Some(id) = other.list_id() {
                        params.set("view", id);
                    }
                }
            }
            if let Some(id) = sel {
                params.set("book", &id);
            }
            if rd.is_some() {
                params.set("read", "1");
            }
            if let Ok(history) = window().history() {
                let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url.href()));
            }
        }
    });

    // Auto-dismiss toast notice after 6s.
    let notice_seq = RwSignal::new(0u32);
    Effect::new(move |_| {
        if notice.with(Option::is_none) {
            return;
        }
        let id = notice_seq.get_untracked() + 1;
        notice_seq.set(id);
        spawn_local(async move {
            crate::sleep(6000).await;
            if notice_seq.try_get_untracked() == Some(id) {
                let _ = notice.try_set(None);
            }
        });
    });

    // Toggle `is-reading-book` class on `document.body`.
    Effect::new(move |_| {
        let active = reading.with(Option::is_some);
        if let Some(body) = document().body() {
            let _ = body
                .class_list()
                .toggle_with_force("is-reading-book", active);
        }
    });
    on_cleanup(|| {
        if let Some(body) = document().body() {
            let _ = body.class_list().remove_1("is-reading-book");
        }
    });

    let patch_book = Callback::new(move |(id, state): (String, Option<BookState>)| {
        let _ = library.try_update(|slot| {
            if let Some(lib) = slot {
                for b in &mut lib.books {
                    if b.id == id {
                        b.state = state.clone();
                    }
                }
            }
        });
    });

    // Save a partial preferences patch sequentially (with optimistic local update and 412 retry).
    let save_prefs = Callback::new(move |patch: Map<String, Value>| {
        // Optimistic update so reader typography/theme reacts immediately.
        shared.preferences.value.update(|slot| {
            let prefs = slot.get_or_insert_with(Preferences::default);
            for (k, v) in &patch {
                prefs.rest.insert(k.clone(), v.clone());
            }
        });
        let queue = pref_queue.get_value();
        spawn_local(async move {
            let _turn = queue.lock().await;
            let Some(sess) = session_sv.try_get_value() else {
                return;
            };
            let rev = shared
                .preferences
                .value
                .try_with_untracked(|p| p.as_ref().map_or(0, |p| p.revision))
                .unwrap_or(0);
            let make_body = |r: u64| {
                let mut map = patch.clone();
                map.insert("revision".into(), json!(r));
                Value::Object(map)
            };
            let mut result = save_preferences(make_body(rev), rev, &sess).await;
            if let Err(ref e) = result
                && e.status == 412
                && let Ok(latest) = api::get::<Preferences>(PREFERENCES).await
            {
                result = save_preferences(make_body(latest.revision), latest.revision, &sess).await;
            }
            match result {
                Ok(next) => {
                    let _ = shared.preferences.value.try_set(Some(next));
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                }
            }
        });
    });

    let set_status = Callback::new(
        move |(target, next_status, undoable): (BookItem, &'static str, bool)| {
            let before = target.state.clone();
            let csrf = session_sv.with_value(|s| s.csrf_token.clone());
            let id = target.id.clone();
            spawn_local(async move {
                let rev = before.as_ref().map_or(0, |s| s.revision);
                let mut body = Map::new();
                body.insert("revision".into(), json!(rev));
                body.insert("status".into(), json!(next_status));
                if next_status == "finished" {
                    body.insert("progress".into(), json!(1));
                }
                match api::send::<BookState>(
                    &format!("{API}/{id}/state"),
                    "PUT",
                    Some(Value::Object(body)),
                    &csrf,
                )
                .await
                {
                    Ok(state) => {
                        patch_book.run((id.clone(), Some(state.clone())));
                        if undoable {
                            let msg = if next_status.is_empty() {
                                "Removed from your lists".to_owned()
                            } else {
                                format!("Marked as {}", status_label(next_status).to_lowercase())
                            };
                            let state_rev = state.revision;
                            let before_status = before
                                .as_ref()
                                .map_or_else(String::new, |s| s.status.clone());
                            let before_progress = before.as_ref().map_or(0.0, |s| s.progress);
                            let undo_id = id.clone();
                            let undo_csrf = csrf.clone();
                            let undo: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                                let undo_id = undo_id.clone();
                                let undo_csrf = undo_csrf.clone();
                                let before_status = before_status.clone();
                                spawn_local(async move {
                                    let body = json!({
                                        "revision": state_rev,
                                        "status": before_status,
                                        "progress": before_progress,
                                    });
                                    match api::send::<BookState>(
                                        &format!("{API}/{undo_id}/state"),
                                        "PUT",
                                        Some(body),
                                        &undo_csrf,
                                    )
                                    .await
                                    {
                                        Ok(val) => patch_book.run((undo_id, Some(val))),
                                        Err(e) => {
                                            let _ = error.try_set(e.message);
                                        }
                                    }
                                });
                            });
                            let _ = notice.try_set(Some(Notice {
                                message: msg,
                                undo: Some(undo),
                            }));
                        }
                    }
                    Err(e) if e.status == 409 => {
                        let _ = reload.try_update(|n| *n += 1);
                        let _ =
                            error.try_set("This book changed on another device. Try again.".into());
                    }
                    Err(e) => {
                        let _ = error.try_set(e.message);
                    }
                }
            });
        },
    );

    let upload_files = move |files: web_sys::FileList| {
        let len = files.length();
        if len == 0 {
            return;
        }
        let Some(lib) = library.get_untracked() else {
            return;
        };
        let parent_id = match view.get_untracked() {
            View::Library(id) => id,
            _ => lib.root_id.clone(),
        };
        let mut taken: HashSet<String> = lib
            .books
            .iter()
            .filter(|b| b.parent_id == parent_id)
            .map(|b| b.name.to_lowercase())
            .collect();
        let file_vec: Vec<web_sys::File> = (0..len).filter_map(|i| files.get(i)).collect();
        let csrf = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            for file in file_vec {
                let raw_name = file.name();
                let id = crate::random_uuid();
                let Some(format) = book_format(&raw_name) else {
                    let _ = transfers.try_update(|t| {
                        t.push(BookTransfer {
                            id,
                            name: raw_name,
                            progress: 0.0,
                            status: "error",
                            message: Some(
                                "Books opens EPUB, PDF and comic archives (CBZ, CBR, CB7, CBT)."
                                    .into(),
                            ),
                        });
                    });
                    continue;
                };
                let name = dedupe_filename(&raw_name, &taken);
                taken.insert(name.to_lowercase());
                let _ = transfers.try_update(|t| {
                    t.push(BookTransfer {
                        id: id.clone(),
                        name: name.clone(),
                        progress: 0.0,
                        status: "uploading",
                        message: None,
                    });
                });
                let body = json!({
                    "name": name,
                    "parentId": parent_id,
                    "size": file.size(),
                    "mediaType": media_type(format),
                });
                let res = async {
                    let reservation: UploadReservation =
                        api::send("/api/v1/files/uploads", "POST", Some(body), &csrf)
                            .await
                            .map_err(|e| e.message)?;
                    let tid = id.clone();
                    upload_blob_xhr(
                        &format!("/api/v1/files/uploads/{}", reservation.id),
                        &csrf,
                        &file,
                        move |p| {
                            let _ = transfers.try_update(|t| {
                                if let Some(item) = t.iter_mut().find(|x| x.id == tid) {
                                    item.progress = p;
                                }
                            });
                        },
                    )
                    .await
                }
                .await;
                match res {
                    Ok(()) => {
                        let _ = transfers.try_update(|t| {
                            if let Some(item) = t.iter_mut().find(|x| x.id == id) {
                                item.progress = 1.0;
                                item.status = "complete";
                            }
                        });
                    }
                    Err(msg) => {
                        let _ = transfers.try_update(|t| {
                            if let Some(item) = t.iter_mut().find(|x| x.id == id) {
                                item.status = "error";
                                item.message = Some(msg);
                            }
                        });
                    }
                }
            }
            let _ = reload.try_update(|n| *n += 1);
        });
    };

    let book_actions = move |target: BookItem| -> Vec<MenuAction> {
        let has_position = target
            .state
            .as_ref()
            .and_then(|s| s.position.as_ref())
            .is_some();
        let cur_status = target
            .state
            .as_ref()
            .map(|s| s.status.as_str())
            .unwrap_or("");
        let id_read = target.id.clone();
        let id_det = target.id.clone();
        let t_want = target.clone();
        let want_next = if cur_status == "want" { "" } else { "want" };
        let t_fin = target.clone();
        let fin_next = if cur_status == "finished" {
            ""
        } else {
            "finished"
        };
        let t_trash = target.clone();
        vec![
            MenuAction::new(
                if has_position {
                    "Continue reading"
                } else {
                    "Read"
                },
                "reading",
            )
            .disabled(!target.error.is_empty())
            .run(move || {
                selected.set(Some(id_read.clone()));
                reading.set(Some(id_read.clone()));
            }),
            MenuAction::new("Details", "info").run(move || {
                selected.set(Some(id_det.clone()));
            }),
            MenuAction::new(
                if cur_status == "want" {
                    "Remove from Want to read"
                } else {
                    "Want to read"
                },
                "bookmark",
            )
            .separator()
            .run(move || set_status.run((t_want.clone(), want_next, true))),
            MenuAction::new(
                if cur_status == "finished" {
                    "Mark as unread"
                } else {
                    "Mark as finished"
                },
                "check",
            )
            .run(move || set_status.run((t_fin.clone(), fin_next, true))),
            MenuAction::new("Download", "download")
                .href(format!("/api/v1/files/{}/content", target.id))
                .download()
                .separator(),
            MenuAction::new("Show in Files", "folder")
                .href(format!("/?folder={}&file={}", target.parent_id, target.id)),
            MenuAction::new("Move to deleted files", "trash")
                .danger()
                .separator()
                .run(move || confirm.set(Some(t_trash.clone()))),
        ]
    };

    let open_id = Memo::new(move |_| {
        let r = reading.get()?;
        library.with(|l| l.as_ref()?.books.iter().any(|b| b.id == r).then_some(r))
    });
    let details_id = Memo::new(move |_| {
        if open_id.get().is_some() {
            return None;
        }
        let s = selected.get()?;
        library.with(|l| l.as_ref()?.books.iter().any(|b| b.id == s).then_some(s))
    });

    let title = Memo::new(move |_| match view.get() {
        View::Library(id) => library.with(|l| {
            l.as_ref()
                .and_then(|l| l.libraries.iter().find(|f| f.id == id))
                .map(|f| f.name.clone())
                .unwrap_or_else(|| "Library".into())
        }),
        other => {
            let id = other.list_id().unwrap_or("home");
            VIEWS
                .iter()
                .find(|(vid, ..)| *vid == id)
                .map(|(_, label, _)| (*label).to_owned())
                .unwrap_or_else(|| "Home".into())
        }
    });

    let filtered_list = Memo::new(move |_| {
        let v = view.get();
        library.with(|l| {
            let Some(lib) = l else { return Vec::new() };
            match &v {
                View::Library(id) => lib
                    .books
                    .iter()
                    .filter(|b| b.library == *id)
                    .cloned()
                    .collect(),
                View::Reading => lib
                    .books
                    .iter()
                    .filter(|b| b.state.as_ref().is_some_and(|s| s.status == "reading"))
                    .cloned()
                    .collect(),
                View::Want => lib
                    .books
                    .iter()
                    .filter(|b| b.state.as_ref().is_some_and(|s| s.status == "want"))
                    .cloned()
                    .collect(),
                View::Finished => lib
                    .books
                    .iter()
                    .filter(|b| b.state.as_ref().is_some_and(|s| s.status == "finished"))
                    .cloned()
                    .collect(),
                View::Home | View::All => lib.books.clone(),
            }
        })
    });

    let choose = move |next: View| {
        view.set(next);
        navigation.set(false);
    };

    let trigger_upload = move |_| {
        if let Some(input) = file_input.get_untracked() {
            input.click();
        }
    };

    let on_close_reader = Callback::new(move |_| {
        reading.set(None);
        reload.update(|n| *n += 1);
    });

    let shell_class = move || {
        format!(
            "app-shell pim-shell books-shell{}{}",
            if details_id.with(Option::is_some) {
                " has-pim-details"
            } else {
                ""
            },
            if open_id.with(Option::is_some) {
                " is-reading"
            } else {
                ""
            },
        )
    };

    view! {
        <div class=shell_class>
            <WorkspaceHeader
                app="books"
                session=session_sv.get_value()
                on_signed_out=on_signed_out
                on_settings=Callback::new(move |_| settings.set(true))
            />
            <div class="pim-workspace books-workspace">
                <Show when=move || navigation.get()>
                    <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| navigation.set(false)/>
                </Show>
                <aside
                    id=navigation_id.clone()
                    node_ref=navigation_ref
                    class=move || if navigation.get() { "sidebar books-sidebar is-open" } else { "sidebar books-sidebar" }
                    aria-label="Books navigation"
                >
                    <button
                        type="button"
                        class="new-button ui-new"
                        disabled=move || library.with(Option::is_none)
                        on:click=trigger_upload
                    >
                        <Icon name="plus"/>"Add books"
                    </button>
                    <input
                        node_ref=file_input
                        type="file"
                        multiple
                        hidden
                        accept=ACCEPT
                        aria-label="Add books"
                        on:change=move |ev| {
                            if let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                if let Some(files) = target.files() {
                                    upload_files(files);
                                }
                                target.set_value("");
                            }
                        }
                    />
                    <nav aria-label="Books views">
                        {VIEWS.iter().map(|&(id, label, icon)| {
                            let target_view = View::from_list_id(id).unwrap_or(View::Home);
                            let tv_class = target_view.clone();
                            let tv_curr = target_view.clone();
                            let tv_click = target_view;
                            let count = move || {
                                if id == "home" {
                                    return 0;
                                }
                                library.with(|l| {
                                    let Some(lib) = l else { return 0 };
                                    match id {
                                        "reading" => lib.books.iter().filter(|b| b.state.as_ref().is_some_and(|s| s.status == "reading")).count(),
                                        "want" => lib.books.iter().filter(|b| b.state.as_ref().is_some_and(|s| s.status == "want")).count(),
                                        "finished" => lib.books.iter().filter(|b| b.state.as_ref().is_some_and(|s| s.status == "finished")).count(),
                                        "all" => lib.books.len(),
                                        _ => 0,
                                    }
                                })
                            };
                            view! {
                                <button
                                    class=move || if view.get() == tv_class { "nav-item active" } else { "nav-item" }
                                    aria-current=move || (view.get() == tv_curr).then_some("page")
                                    on:click=move |_| choose(tv_click.clone())
                                >
                                    <Icon name=icon/>
                                    <span class="books-row-title">{label}</span>
                                    {move || {
                                        let c = count();
                                        (c > 0).then(|| view! { <span class="books-count">{c}</span> })
                                    }}
                                </button>
                            }
                        }).collect_view()}
                    </nav>
                    <div class="sidebar-heading books-heading"><h2>"Libraries"</h2></div>
                    <nav aria-label="Libraries" class="books-libraries">
                        {move || {
                            let Some(lib) = library.get() else {
                                return ().into_any();
                            };
                            if lib.libraries.is_empty() {
                                view! { <p class="books-hint">{format!("Folders in Files \u{203a} {} become libraries.", lib.path)}</p> }.into_any()
                            } else {
                                lib.libraries.into_iter().map(|folder| {
                                    let fid1 = folder.id.clone();
                                    let fid2 = folder.id.clone();
                                    let fid3 = folder.id.clone();
                                    let count = folder.count;
                                    view! {
                                        <button
                                            class=move || if view.get() == View::Library(fid1.clone()) { "nav-item active" } else { "nav-item" }
                                            aria-current=move || (view.get() == View::Library(fid2.clone())).then_some("page")
                                            on:click=move |_| choose(View::Library(fid3.clone()))
                                        >
                                            <Icon name="library"/>
                                            <span class="books-row-title">{folder.name}</span>
                                            {(count > 0).then(|| view! { <span class="books-count">{count}</span> })}
                                        </button>
                                    }
                                }).collect_view().into_any()
                            }
                        }}
                    </nav>
                    {move || {
                        let n = library.with(|l| l.as_ref().map_or(0, |l| l.indexing));
                        (n > 0).then(|| view! {
                            <p class="books-indexing" role="status">
                                <span class="books-spinner" aria-hidden="true"></span>
                                {format!("Reading {n} new {}\u{2026}", if n == 1 { "book" } else { "books" })}
                            </p>
                        })
                    }}
                    <AppSettingsButton app="books" on_click=Callback::new(move |_| settings.set(true))/>
                </aside>

                {move || {
                    if let Some(id) = open_id.get() {
                        let book = library.with_untracked(|l| {
                            l.as_ref().and_then(|l| l.books.iter().find(|b| b.id == id).cloned())
                        });
                        if let Some(book) = book {
                            let id_for_actions = id.clone();
                            let id_for_state = id.clone();
                            let jump = deep_page
                                .try_update_value(|slot| slot.take_if(|(b, _)| *b == id))
                                .flatten()
                                .map(|(_, page)| page);
                            return view! {
                                <Reader
                                    book=book
                                    start_page=jump
                                    session=session_sv.get_value()
                                    on_preferences=save_prefs
                                    on_close=on_close_reader
                                    on_state=Callback::new(move |st: BookState| patch_book.run((id_for_state.clone(), Some(st))))
                                    actions=Callback::new(move |_| {
                                        library
                                            .with_untracked(|l| {
                                                l.as_ref().and_then(|l| l.books.iter().find(|b| b.id == id_for_actions).cloned())
                                            })
                                            .map(|b| book_actions(b).into_iter().skip(2).collect())
                                            .unwrap_or_default()
                                    })
                                />
                            }
                            .into_any();
                        }
                    }

                    let sort_actions = Callback::new(move |_| {
                        let cur = sort.get_untracked();
                        SORTS
                            .iter()
                            .map(|&(val, label)| {
                                MenuAction::plain(label).current(cur == val).run(move || {
                                    let mut map = Map::new();
                                    map.insert("booksSort".into(), json!(val));
                                    save_prefs.run(map);
                                })
                            })
                            .collect::<Vec<_>>()
                    });

                    view! {
                        <main class="pim-main books-main" tabindex="-1" aria-label=move || title.get()>
                            <div class="books-heading-bar">
                                <h1>{move || title.get()}</h1>
                                {move || {
                                    (library.with(Option::is_some) && view.get() != View::Home).then(|| view! {
                                        <span class="books-heading-count">{move || filtered_list.with(Vec::len)}</span>
                                    })
                                }}
                                <div class="books-tools">
                                    {move || {
                                        let show = view.get() != View::Home
                                            && library.with(|l| l.as_ref().is_some_and(|l| !l.books.is_empty()));
                                        show.then(|| view! {
                                            <ActionMenu label="Sort books" class="workspace-sort" actions=sort_actions>
                                                <Icon name="sort"/>
                                                <span>{move || {
                                                    let cur = sort.get();
                                                    SORTS.iter().find(|(v, _)| *v == cur).map(|(_, l)| *l).unwrap_or("Recently read")
                                                }}</span>
                                            </ActionMenu>
                                        })
                                    }}
                                </div>
                            </div>
                            {move || {
                                let err = error.get();
                                (!err.is_empty() && library.with(Option::is_some)).then(|| view! {
                                    <div role="alert" class="error books-error">
                                        {err}
                                        <button class="text-button" on:click=move |_| { error.set(String::new()); reload.update(|n| *n += 1); }>"Reload"</button>
                                    </div>
                                })
                            }}
                            {move || {
                                let err = error.get();
                                let lib_opt = library.get();
                                if !err.is_empty() && lib_opt.is_none() {
                                    return view! {
                                        <div class="books-empty" role="alert">
                                            <h2>"Books could not be loaded"</h2>
                                            <p>{err}</p>
                                            <button class="secondary-button" on:click=move |_| reload.update(|n| *n += 1)>"Try again"</button>
                                        </div>
                                    }.into_any();
                                }
                                let Some(lib) = lib_opt else {
                                    return view! { <p class="books-empty" role="status">"Loading your library\u{2026}"</p> }.into_any();
                                };
                                if lib.books.is_empty() {
                                    return view! {
                                        <div class="books-empty">
                                            <span class="empty-icon"><Icon name="book"/></span>
                                            <h2>{format!("No books in {} yet", lib.path)}</h2>
                                            <p>"Add EPUB, PDF or comic files (CBZ, CBR), or move them into this folder in Files."</p>
                                            <div class="books-empty-actions">
                                                <button class="primary-button" on:click=trigger_upload><Icon name="upload"/>"Add books"</button>
                                                <a class="secondary-button" href=format!("/?folder={}", lib.root_id)>"Open in Files"</a>
                                            </div>
                                        </div>
                                    }.into_any();
                                }
                                let cur_view = view.get();
                                let cur_sort = sort.get();
                                if cur_view == View::Home {
                                    let reading_books: Vec<BookItem> = lib.books.iter().filter(|b| b.state.as_ref().is_some_and(|s| s.status == "reading")).cloned().collect();
                                    let continuing: Vec<BookItem> = sort_books(&reading_books, &cur_sort).into_iter().take(6).collect();
                                    let mut added = lib.books.clone();
                                    added.sort_by(|a, b| b.modified_at.total_cmp(&a.modified_at));
                                    added.truncate(16);
                                    let want_books: Vec<BookItem> = lib.books.iter().filter(|b| b.state.as_ref().is_some_and(|s| s.status == "want")).cloned().collect();
                                    let wanted: Vec<BookItem> = sort_books(&want_books, &cur_sort).into_iter().take(16).collect();
                                    let path_hint = format!("in {}", lib.path);
                                    return view! {
                                        <div class="books-scroll">
                                            {(!continuing.is_empty()).then(|| view! {
                                                <section aria-label="Continue reading">
                                                    <div class="books-shelf-head">
                                                        <h2>"Continue reading"</h2>
                                                        <button class="text-button" on:click=move |_| choose(View::Reading)>"See all"</button>
                                                    </div>
                                                    <div class="books-continue">
                                                        {continuing.into_iter().map(|b| {
                                                            let bid = b.id.clone();
                                                            let bid_sel = bid.clone();
                                                            let bid_open = bid.clone();
                                                            let bid_det = bid.clone();
                                                            view! {
                                                                <ContinueCard
                                                                    book=b
                                                                    selected=Signal::derive(move || selected.get().as_deref() == Some(bid_sel.as_str()))
                                                                    on_open=Callback::new(move |_| {
                                                                        selected.set(Some(bid_open.clone()));
                                                                        reading.set(Some(bid_open.clone()));
                                                                    })
                                                                    on_details=Callback::new(move |_| selected.set(Some(bid_det.clone())))
                                                                />
                                                            }
                                                        }).collect_view()}
                                                    </div>
                                                </section>
                                            })}
                                            <Shelf
                                                title="Recently added"
                                                hint=Some(path_hint)
                                                books=added
                                                selected=selected
                                                on_select=Callback::new(move |id: String| selected.set(Some(id)))
                                                on_all=Callback::new(move |_| choose(View::All))
                                                actions=Callback::new(move |b: BookItem| book_actions(b))
                                            />
                                            {(!wanted.is_empty()).then(|| view! {
                                                <Shelf
                                                    title="Want to read"
                                                    hint=None
                                                    books=wanted
                                                    selected=selected
                                                    on_select=Callback::new(move |id: String| selected.set(Some(id)))
                                                    on_all=Callback::new(move |_| choose(View::Want))
                                                    actions=Callback::new(move |b: BookItem| book_actions(b))
                                                />
                                            })}
                                        </div>
                                    }.into_any();
                                }

                                let items = filtered_list.get();
                                if items.is_empty() {
                                    return match &cur_view {
                                        View::Library(id) => {
                                            let lib_name = lib.libraries.iter().find(|f| f.id == *id).map(|f| f.name.as_str()).unwrap_or("");
                                            view! {
                                                <div class="books-empty">
                                                    <span class="empty-icon"><Icon name="library"/></span>
                                                    <h2>"This library is empty"</h2>
                                                    <p>{format!("Add books here, or move them into Files \u{203a} {} \u{203a} {lib_name}.", lib.path)}</p>
                                                    <button class="primary-button" on:click=trigger_upload><Icon name="upload"/>"Add books"</button>
                                                </div>
                                            }.into_any()
                                        }
                                        View::Reading => view! {
                                            <div class="books-empty">
                                                <span class="empty-icon"><Icon name="reading"/></span>
                                                <h2>"Nothing in progress"</h2>
                                                <p>"Books you start reading appear here."</p>
                                            </div>
                                        }.into_any(),
                                        View::Want => view! {
                                            <div class="books-empty">
                                                <span class="empty-icon"><Icon name="bookmark"/></span>
                                                <h2>"Your list is empty"</h2>
                                                <p>"Choose Want to read on a book to keep it here."</p>
                                            </div>
                                        }.into_any(),
                                        _ => view! {
                                            <div class="books-empty">
                                                <span class="empty-icon"><Icon name="check"/></span>
                                                <h2>"No finished books"</h2>
                                                <p>"Books you finish, or mark as finished, appear here."</p>
                                            </div>
                                        }.into_any(),
                                    };
                                }

                                let ordered = sort_books(&items, &cur_sort);
                                let grid_label = title.get_untracked();
                                let truncated = lib.truncated;
                                view! {
                                    <div class="books-scroll">
                                        <ul class="books-grid" aria-label=grid_label>
                                            {ordered.into_iter().map(|b| {
                                                let bid = b.id.clone();
                                                let bid_sel = bid.clone();
                                                let bid_click = bid.clone();
                                                let b_act = b.clone();
                                                view! {
                                                    <li>
                                                        <BookCard
                                                            book=b
                                                            selected=Signal::derive(move || selected.get().as_deref() == Some(bid_sel.as_str()))
                                                            on_select=Callback::new(move |_| selected.set(Some(bid_click.clone())))
                                                            actions=Callback::new(move |_| book_actions(b_act.clone()))
                                                        />
                                                    </li>
                                                }
                                            }).collect_view()}
                                        </ul>
                                        {truncated.then(|| view! { <p class="books-hint">"This folder is very large; only the first books are shown."</p> })}
                                    </div>
                                }.into_any()
                            }}
                        </main>
                    }
                    .into_any()
                }}

                {move || {
                    let id = details_id.get()?;
                    let id_read = id.clone();
                    let id_status = id.clone();
                    let id_actions = id.clone();
                    Some(view! {
                        <aside class="pim-details books-details" tabindex="-1" aria-label="Book details">
                            <BookDetails
                                book_id=id
                                library=library
                                on_close=Callback::new(move |_| selected.set(None))
                                on_read=Callback::new(move |_| reading.set(Some(id_read.clone())))
                                on_status=Callback::new(move |next_status: &'static str| {
                                    if let Some(b) = library.with_untracked(|l| l.as_ref().and_then(|l| l.books.iter().find(|b| b.id == id_status).cloned())) {
                                        set_status.run((b, next_status, true));
                                    }
                                })
                                on_select=Callback::new(move |next_id: String| selected.set(Some(next_id)))
                                actions=Callback::new(move |_| {
                                    library
                                        .with_untracked(|l| l.as_ref().and_then(|l| l.books.iter().find(|b| b.id == id_actions).cloned()))
                                        .map(|b| book_actions(b).into_iter().skip(1).collect())
                                        .unwrap_or_default()
                                })
                            />
                        </aside>
                    })
                }}
            </div>

            {move || {
                let list = transfers.get();
                (!list.is_empty()).then(|| {
                    let uploading_count = list.iter().filter(|t| t.status == "uploading").count();
                    let any_uploading = uploading_count > 0;
                    let heading = if any_uploading {
                        format!("Adding {uploading_count}")
                    } else {
                        "Added books".into()
                    };
                    view! {
                        <section class="transfer-panel books-transfers" aria-label="Uploads">
                            <div class="transfer-heading">
                                <strong>{heading}</strong>
                                <button
                                    class="icon-button"
                                    aria-label="Dismiss finished uploads"
                                    disabled=any_uploading
                                    on:click=move |_| transfers.set(Vec::new())
                                >
                                    <Icon name="close"/>
                                </button>
                            </div>
                            <ul>
                                {list.into_iter().map(|t| {
                                    let prog = t.progress;
                                    let is_complete = t.status == "complete";
                                    view! {
                                        <li class=format!("books-transfer is-{}", t.status)>
                                            <span>{t.name.clone()}</span>
                                            <progress max="1" value=prog.to_string() aria-label=format!("{} upload progress", t.name)/>
                                            {match t.message {
                                                Some(msg) => view! { <small role="alert">{msg}</small> }.into_any(),
                                                None => view! { <small>{if is_complete { "Done".into() } else { percent(prog) }}</small> }.into_any(),
                                            }}
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        </section>
                    }
                })
            }}

            {move || {
                confirm.get().map(|target| {
                    let target_for_dialog = StoredValue::new(target);
                    view! {
                        <Dialog
                            title="Move book to deleted files?"
                            on_close=Callback::new(move |_| confirm.set(None))
                        >
                            {move || {
                                let b = target_for_dialog.get_value();
                                view! {
                                    <ConfirmTrash
                                        book=b
                                        live=live
                                        on_cancel=Callback::new(move |_| confirm.set(None))
                                        on_done=Callback::new(move |(id, title): (String, String)| {
                                            if selected.get_untracked().as_deref() == Some(id.as_str()) {
                                                selected.set(None);
                                            }
                                            if reading.get_untracked().as_deref() == Some(id.as_str()) {
                                                reading.set(None);
                                            }
                                            confirm.set(None);
                                            notice.set(Some(Notice {
                                                message: format!("Moved \u{201c}{title}\u{201d} to deleted files"),
                                                undo: None,
                                            }));
                                            reload.update(|n| *n += 1);
                                        })
                                    />
                                }
                            }}
                        </Dialog>
                    }
                })
            }}

            {move || {
                settings.get().then(|| view! {
                    <BooksSettings
                        session=session_sv.get_value()
                        library=library
                        on_close=Callback::new(move |_| {
                            settings.set(false);
                            reload.update(|n| *n += 1);
                        })
                    />
                })
            }}

            <div class="books-toast-region" role="status" aria-live="polite">
                {move || {
                    notice.get().map(|n| view! {
                        <div class="settings-toast books-toast">
                            <span>{n.message.clone()}</span>
                            {n.undo.map(|undo| view! {
                                <button
                                    type="button"
                                    class="text-button"
                                    on:click=move |_| {
                                        notice.set(None);
                                        undo();
                                    }
                                >"Undo"</button>
                            })}
                        </div>
                    })
                }}
            </div>
        </div>
    }
}

#[component]
fn ConfirmTrash(
    book: BookItem,
    live: Live,
    on_cancel: Callback<()>,
    on_done: Callback<(String, String)>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let err = RwSignal::new(String::new());
    let book_sv = StoredValue::new(book);
    let on_confirm = move |_| {
        busy.set(true);
        let b = book_sv.get_value();
        spawn_local(async move {
            let Some(conn) = live.connection.get_untracked() else {
                let _ = err.try_set("Books is reconnecting. Try again in a moment.".into());
                let _ = busy.try_set(false);
                return;
            };
            let id = b.id.clone();
            match reducer(|tx| conn.reducers.trash_entry_then(id, b.revision, replied(tx))).await {
                Ok(()) => {
                    on_done.try_run((b.id, b.title));
                }
                Err(msg) => {
                    let _ = err.try_set(msg);
                    let _ = busy.try_set(false);
                }
            }
        });
    };
    let text = book_sv.with_value(|b| {
        format!(
            "\u{201c}{}\u{201d} ({}) moves to Files \u{203a} Deleted files, where it can be restored. Your reading progress is kept.",
            b.title, b.name
        )
    });
    view! {
        <p>{text}</p>
        {move || (!err.get().is_empty()).then(|| view! { <p class="error" role="alert">{err.get()}</p> })}
        <div class="dialog-actions">
            <button class="text-button" on:click=move |_| { on_cancel.try_run(()); }>"Cancel"</button>
            <button class="danger-button" disabled=move || busy.get() on:click=on_confirm>"Move to deleted files"</button>
        </div>
    }
}

/// The cover image, or a typographic cover when the book has none or it cannot load.
#[component]
fn Cover(book: BookItem, #[prop(default = "m")] size: &'static str) -> impl IntoView {
    let failed = RwSignal::new(!book.cover);
    let done = book.state.as_ref().is_some_and(|s| s.status == "finished");
    let progress = book
        .state
        .as_ref()
        .filter(|s| s.status == "reading")
        .map_or(0.0, |s| s.progress);
    let title = book.title.clone();
    let title_hue = hue(&book.title);
    let first_author = book.authors.first().cloned().unwrap_or_default();
    let cover_src = format!("{API}/{}/cover?r={}", book.id, book.revision);
    let format = book.format.clone();
    let show_format = format != "epub" && size != "s";
    let format_label = book_format_label(&format);

    view! {
        <span class=move || {
            if failed.get() {
                format!("books-cover is-{size} is-generated hue-{title_hue}")
            } else {
                format!("books-cover is-{size}")
            }
        }>
            {move || {
                if failed.get() {
                    view! {
                        <b>{title.clone()}</b>
                        <i>{first_author.clone()}</i>
                    }.into_any()
                } else {
                    view! {
                        <img
                            src=cover_src.clone()
                            alt=""
                            loading="lazy"
                            decoding="async"
                            on:error=move |_| failed.set(true)
                        />
                    }.into_any()
                }
            }}
            {show_format.then(|| view! { <em class="books-format">{format_label.clone()}</em> })}
            {(progress > 0.0 && progress < 1.0).then(|| view! {
                <span class="books-progress">
                    <progress max="1" value=progress.to_string() aria-label=format!("{} read", percent(progress))/>
                </span>
            })}
            {done.then(|| view! {
                <span class="books-done" title="Finished">
                    <Icon name="check"/>
                    <span class="sr-only">"Finished"</span>
                </span>
            })}
        </span>
    }
}

#[component]
fn BookCard(
    book: BookItem,
    #[prop(into)] selected: Signal<bool>,
    on_select: Callback<()>,
    actions: Callback<(), Vec<MenuAction>>,
) -> impl IntoView {
    let title = book.title.clone();
    let subtitle = if !book.series.is_empty() {
        match book.series_index {
            Some(idx) => format!("{} \u{00b7} {idx}", book.series),
            None => book.series.clone(),
        }
    } else {
        authors(&book)
    };
    let menu_label = format!("Actions for {}", book.title);
    view! {
        <div class=move || if selected.get() { "books-card is-selected" } else { "books-card" }>
            <button
                type="button"
                class="books-card-open"
                aria-pressed=move || selected.get().to_string()
                on:click=move |_| { on_select.try_run(()); }
            >
                <Cover book=book/>
                <strong>{title}</strong>
                <small>{subtitle}</small>
            </button>
            <ActionMenu label=menu_label class="books-card-menu" actions=actions>
                <Icon name="more"/>
            </ActionMenu>
        </div>
    }
}

#[component]
fn Shelf(
    #[prop(into)] title: String,
    hint: Option<String>,
    books: Vec<BookItem>,
    selected: RwSignal<Option<String>>,
    on_select: Callback<String>,
    on_all: Callback<()>,
    actions: Callback<BookItem, Vec<MenuAction>>,
) -> impl IntoView {
    view! {
        <section aria-label=title.clone()>
            <div class="books-shelf-head">
                <h2>{title.clone()}</h2>
                {hint.map(|h| view! { <small>{h}</small> })}
                <button class="text-button" on:click=move |_| { on_all.try_run(()); }>"See all"</button>
            </div>
            <ul class="books-shelf">
                {books.into_iter().map(|b| {
                    let bid = b.id.clone();
                    let bid_sel = bid.clone();
                    let bid_click = bid.clone();
                    let b_act = b.clone();
                    view! {
                        <li>
                            <BookCard
                                book=b
                                selected=Signal::derive(move || selected.get().as_deref() == Some(bid_sel.as_str()))
                                on_select=Callback::new(move |_| { on_select.try_run(bid_click.clone()); })
                                actions=Callback::new(move |_| actions.run(b_act.clone()))
                            />
                        </li>
                    }
                }).collect_view()}
            </ul>
        </section>
    }
}

#[component]
fn ContinueCard(
    book: BookItem,
    #[prop(into)] selected: Signal<bool>,
    on_open: Callback<()>,
    on_details: Callback<()>,
) -> impl IntoView {
    let progress = book.state.as_ref().map_or(0.0, |s| s.progress);
    let title = book.title.clone();
    let auth = authors(&book);
    let place_text = place(&book);
    let updated_text = book
        .state
        .as_ref()
        .filter(|s| s.updated_at > 0.0)
        .map(|s| edited(s.updated_at))
        .unwrap_or_default();
    let details_label = format!("Details for {}", book.title);
    view! {
        <div class=move || if selected.get() { "books-continue-card is-selected" } else { "books-continue-card" }>
            <button
                type="button"
                class="books-continue-cover"
                aria-label=details_label
                on:click=move |_| { on_details.try_run(()); }
            >
                <Cover book=book size="s"/>
            </button>
            <button
                type="button"
                class="books-continue-body"
                on:click=move |_| { on_open.try_run(()); }
            >
                <strong>{title}</strong>
                <small>{auth}</small>
                <span class="books-continue-place">{place_text}</span>
                <progress max="1" value=progress.to_string() aria-label=format!("{} read", percent(progress))/>
                <span class="books-continue-meta">
                    <span>{updated_text}</span>
                    <span>"Continue"</span>
                </span>
            </button>
        </div>
    }
}

/// The right details sheet: cover, progress, actions, description, series and file facts.
#[component]
fn BookDetails(
    book_id: String,
    library: RwSignal<Option<BookLibrary>>,
    on_close: Callback<()>,
    on_read: Callback<()>,
    on_status: Callback<&'static str>,
    on_select: Callback<String>,
    actions: Callback<(), Vec<MenuAction>>,
) -> impl IntoView {
    let more = RwSignal::new(false);
    let root = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if let Some(node) = root.get() {
            let opts = web_sys::FocusOptions::new();
            opts.set_prevent_scroll(true);
            let _ = node.focus_with_options(&opts);
        }
    });

    let book_id = StoredValue::new(book_id);
    let current_book = Memo::new(move |_| {
        let id = book_id.get_value();
        library.with(|l| {
            l.as_ref()
                .and_then(|l| l.books.iter().find(|b| b.id == id).cloned())
        })
    });

    let on_keydown = move |event: web_sys::KeyboardEvent| {
        if event.key() == "Escape" {
            let in_menu = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|el| el.closest("[role=menu]").ok().flatten())
                .is_some();
            if !in_menu {
                event.prevent_default();
                {
                    on_close.try_run(());
                };
            }
        }
    };

    view! {
        <div class="books-detail" node_ref=root tabindex="-1" on:keydown=on_keydown>
            {move || {
                let Some(book) = current_book.get() else {
                    return ().into_any();
                };
                let series: Vec<BookItem> = if !book.series.is_empty() {
                    let mut list: Vec<BookItem> = library.with(|l| {
                        l.as_ref()
                            .map(|l| l.books.iter().filter(|b| b.series == book.series).cloned().collect())
                            .unwrap_or_default()
                    });
                    list.sort_by(|a, b| {
                        a.series_index
                            .unwrap_or(1e9)
                            .total_cmp(&b.series_index.unwrap_or(1e9))
                            .then_with(|| locale_cmp(&a.title, &b.title))
                    });
                    list
                } else {
                    Vec::new()
                };
                let progress = book.state.as_ref().map_or(0.0, |s| s.progress);
                let status = book.state.as_ref().map(|s| s.status.as_str()).unwrap_or("");
                let has_position = book.state.as_ref().and_then(|s| s.position.as_ref()).is_some();
                let problem = match book.error.as_str() {
                    "drm" => "This book is protected with DRM by the store it came from, so Dar cannot open it. Download it to read it in that store\u{2019}s app.".to_owned(),
                    "unsupported" => "This archive type could not be opened on this server. Download it to read it elsewhere.".to_owned(),
                    "damaged" => format!("This file could not be read. It may be damaged or not a real {} file.", book_format_label(&book.format)),
                    _ => String::new(),
                };
                let series_line = if !book.series.is_empty() {
                    let mut s = book.series.clone();
                    if let Some(idx) = book.series_index {
                        s.push_str(&format!(" \u{00b7} Book {idx}"));
                    }
                    if series.len() > 1 {
                        s.push_str(&format!(" of {}", series.len()));
                    }
                    Some(s)
                } else {
                    None
                };
                let status_icon = match status {
                    "finished" => "check",
                    "want" => "bookmark",
                    _ => "reading",
                };
                let is_want = status == "want";
                let is_finished = status == "finished";
                let is_reading = status == "reading";
                let read_label = if is_finished {
                    "Read again"
                } else if has_position {
                    "Continue reading"
                } else {
                    "Read"
                };
                let want_next: &'static str = if is_want { "" } else { "want" };
                let fin_next: &'static str = if is_finished { "" } else { "finished" };
                let fin_label = if is_finished { "Mark as unread" } else { "Mark as finished" };
                let more_actions_label = format!("More actions for {}", book.title);
                let desc = book.description.clone();
                let desc_long = desc.len() > 320;
                let desc_lines: Vec<String> = desc.split('\n').map(str::to_owned).collect();
                let current_id = book.id.clone();

                view! {
                    <div class="books-detail-head">
                        <button class="icon-button books-detail-back" aria-label="Close details" title="Close (Escape)" on:click=move |_| { on_close.try_run(()); }><Icon name="back"/></button>
                        <h2>"Details"</h2>
                        <ActionMenu label=more_actions_label actions=actions><Icon name="more"/></ActionMenu>
                        <button class="icon-button books-detail-close" aria-label="Close details" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button>
                    </div>
                    <div class="books-detail-body">
                        <div class="books-hero">
                            <Cover book=book.clone() size="l"/>
                            <div>
                                <h3>{book.title.clone()}</h3>
                                <p>{authors(&book)}</p>
                                {series_line.map(|s| view! { <p class="books-series">{s}</p> })}
                                {(!status.is_empty()).then(|| view! {
                                    <p class="books-status-chip"><Icon name=status_icon/>{status_label(status)}</p>
                                })}
                                {is_reading.then(|| view! {
                                    <progress max="1" value=progress.to_string() aria-label=format!("{} read", percent(progress))/>
                                    <small>{format!("{} \u{00b7} {}", percent(progress), place(&book))}</small>
                                })}
                            </div>
                        </div>
                        {if !problem.is_empty() {
                            view! { <p class="error" role="alert">{problem}</p> }.into_any()
                        } else {
                            view! {
                                <div class="books-quick">
                                    <button class="primary-button" on:click=move |_| { on_read.try_run(()); }><Icon name="reading"/>{read_label}</button>
                                    <button
                                        class=if is_want { "icon-button books-round is-on" } else { "icon-button books-round" }
                                        aria-pressed=is_want.to_string()
                                        aria-label="Want to read"
                                        title="Want to read"
                                        on:click=move |_| { on_status.try_run(want_next); }
                                    ><Icon name="bookmark"/></button>
                                    <a
                                        class="icon-button books-round"
                                        href=format!("/api/v1/files/{}/content", book.id)
                                        download=""
                                        aria-label="Download"
                                        title="Download"
                                    ><Icon name="download"/></a>
                                    <button
                                        class=if is_finished { "icon-button books-round is-on" } else { "icon-button books-round" }
                                        aria-pressed=is_finished.to_string()
                                        aria-label=fin_label
                                        title=fin_label
                                        on:click=move |_| { on_status.try_run(fin_next); }
                                    ><Icon name="check"/></button>
                                </div>
                            }.into_any()
                        }}
                        {(!desc.is_empty()).then(|| view! {
                            <div class=move || if more.get() { "books-description is-open" } else { "books-description" }>
                                {desc_lines.into_iter().map(|line| view! { <p>{line}</p> }).collect_view()}
                                {desc_long.then(|| view! {
                                    <button class="text-button" aria-expanded=move || more.get().to_string() on:click=move |_| more.update(|m| *m = !*m)>
                                        {move || if more.get() { "Less" } else { "More" }}
                                    </button>
                                })}
                            </div>
                        })}
                        {(series.len() > 1).then(|| {
                            let sname = book.series.clone();
                            view! {
                                <section aria-label=format!("{sname} series") class="books-series-strip">
                                    <h4>{sname.clone()}</h4>
                                    <div>
                                        {series.into_iter().map(|b| {
                                            let is_cur = b.id == current_id;
                                            let bid = b.id.clone();
                                            let lbl = match b.series_index {
                                                Some(idx) => format!("{}, book {idx}", b.title),
                                                None => b.title.clone(),
                                            };
                                            view! {
                                                <button
                                                    type="button"
                                                    aria-current=is_cur.then_some("true")
                                                    aria-label=lbl
                                                    on:click=move |_| { on_select.try_run(bid.clone()); }
                                                >
                                                    <Cover book=b size="s"/>
                                                </button>
                                            }
                                        }).collect_view()}
                                    </div>
                                </section>
                            }
                        })}
                        <div class="card books-facts">
                            <div>
                                <Icon name="file"/>
                                <strong>{format!("{} \u{00b7} {}", book_format_label(&book.format), format_bytes(book.size))}</strong>
                                {(book.pages > 0).then(|| view! {
                                    <span>{if book.format == "epub" { format!("\u{2248} {} pages", book.pages) } else { format!("{} pages", book.pages) }}</span>
                                })}
                            </div>
                            {(!book.published.is_empty() || !book.publisher.is_empty()).then(|| view! {
                                <div>
                                    <Icon name="calendar"/>
                                    <strong>{if !book.published.is_empty() { format!("Published {}", book.published) } else { "Publisher".into() }}</strong>
                                    <span>{book.publisher.clone()}</span>
                                </div>
                            })}
                            {(!book.language.is_empty()).then(|| view! {
                                <div>
                                    <Icon name="globe"/>
                                    <strong>{language_display_name(&book.language)}</strong>
                                </div>
                            })}
                            {book.state.as_ref().filter(|s| s.position.is_some()).map(|st| view! {
                                <div>
                                    <Icon name="clock"/>
                                    <strong>{format!("Last read {}", edited(st.updated_at))}</strong>
                                    {(!st.device.is_empty()).then(|| view! { <span>{st.device.clone()}</span> })}
                                </div>
                            })}
                            <div>
                                <Icon name="folder"/>
                                <strong class="books-path">{book.path.clone()}</strong>
                                <a href=format!("/?folder={}&file={}", book.parent_id, book.id)>"Show in Files"</a>
                            </div>
                        </div>
                        {(!book.subjects.is_empty()).then(|| view! {
                            <div class="books-tags">
                                {book.subjects.iter().map(|s| view! { <span class="books-tag">{s.clone()}</span> }).collect_view()}
                            </div>
                        })}
                    </div>
                }.into_any()
            }}
        </div>
    }
}

/// The reader in the middle sheet: a bar, the book, and a footer with the place and a scrubber.
#[component]
fn Reader(
    book: BookItem,
    session: BrowserSession,
    on_preferences: Callback<Map<String, Value>>,
    on_close: Callback<()>,
    on_state: Callback<BookState>,
    actions: Callback<(), Vec<MenuAction>>,
    /// Open at this page (from 0) instead of the saved place (PDF and comics).
    #[prop(default = None)]
    start_page: Option<usize>,
) -> impl IntoView {
    let shared = context();
    // Find in a PDF, and its highlights (kept in the book's note in Notes).
    let find_open = RwSignal::new(false);
    let find_text = RwSignal::new(String::new());
    let find_state: RwSignal<PdfFindState> = RwSignal::new(PdfFindState::default());
    let find_input = NodeRef::<html::Input>::new();
    let highlights: RwSignal<Vec<PdfHighlight>> = RwSignal::new(Vec::new());
    let reader_status = RwSignal::new(String::new());
    let status_seq = StoredValue::new(0u64);
    let show_status = move |message: String| {
        let seq = status_seq.try_get_value().unwrap_or(0) + 1;
        let _ = status_seq.try_set_value(seq);
        let _ = reader_status.try_set(message);
        spawn_local(async move {
            crate::sleep(5000).await;
            if status_seq.try_get_value() == Some(seq) {
                let _ = reader_status.try_set(String::new());
            }
        });
    };
    if book.format == "pdf" {
        let (bid, title) = (book.id.clone(), book.title.clone());
        spawn_local(async move {
            let saved = highlights::load(&bid, &title).await;
            let _ = highlights.try_set(saved);
        });
    }
    let wide = window()
        .match_media("(min-width: 1101px)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches());

    let detail: RwSignal<Option<BookDetail>> = RwSignal::new(None);
    let failed = RwSignal::new(String::new());
    let panel: RwSignal<Option<&'static str>> = RwSignal::new(if wide && book.format == "epub" {
        Some("contents")
    } else {
        None
    });
    let current_where: RwSignal<Option<Where>> = RwSignal::new(None);
    let pdf_info: RwSignal<Option<PdfReady>> = RwSignal::new(None);
    let conflict: RwSignal<Option<Conflict>> = RwSignal::new(None);
    let look: RwSignal<Option<ReaderLook>> = RwSignal::new(None);
    let typography = RwSignal::new(false);
    let marks: RwSignal<Vec<BookMark>> = RwSignal::new(Vec::new());
    let save_error = RwSignal::new(String::new());

    let root = NodeRef::<html::Main>::new();
    let stage = NodeRef::<html::Div>::new();
    let epub_handle: RwSignal<Option<EpubHandle>> = RwSignal::new(None);
    let comic_handle: RwSignal<Option<ComicHandle>> = RwSignal::new(None);
    let pdf_handle: RwSignal<Option<PdfHandle>> = RwSignal::new(None);
    let open_find = move || {
        find_open.set(true);
        spawn_local(async move {
            crate::sleep(0).await;
            if let Some(input) = find_input.try_get_untracked().flatten() {
                let _ = input.focus();
                input.select();
            }
        });
    };
    let close_find = move || {
        find_open.set(false);
        find_text.set(String::new());
        if let Some(h) = pdf_handle.get_untracked() {
            h.find("");
        }
    };
    let aa = NodeRef::<html::Button>::new();
    let aa_panel = NodeRef::<html::Div>::new();

    let revision = StoredValue::new(book.state.as_ref().map_or(0, |s| s.revision));
    let status = StoredValue::new(
        book.state
            .as_ref()
            .map_or_else(String::new, |s| s.status.clone()),
    );
    let pending: StoredValue<Option<Where>> = StoredValue::new(None);
    let save_seq = StoredValue::new(0u64);
    let alive = Arc::new(AtomicBool::new(true));

    let book_sv = StoredValue::new(book.clone());
    let csrf_sv = StoredValue::new(session.csrf_token.clone());
    let session_sv = StoredValue::new(session.clone());

    use_floating(typography.into(), aa, aa_panel, true, 6.0);

    // Fetch book detail & marks.
    {
        let bid = book.id.clone();
        let ctrl = web_sys::AbortController::new().ok();
        let signal = ctrl.as_ref().map(|c| c.signal());
        let held_ctrl = ctrl.map(SendWrapper::new);
        on_cleanup(move || {
            if let Some(c) = held_ctrl {
                c.take().abort();
            }
        });
        spawn_local(async move {
            let aborted = || signal.as_ref().is_some_and(|s| s.aborted());
            match api::request::<BookDetail>(
                &format!("{API}/{bid}"),
                Req {
                    signal: signal.clone(),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(value) => {
                    if !aborted() {
                        let _ = marks.try_set(value.marks.clone());
                        if let Some(st) = &value.state {
                            let _ = revision.try_set_value(st.revision);
                            let _ = status.try_set_value(st.status.clone());
                        }
                        let _ = detail.try_set(Some(value));
                    }
                }
                Err(e) => {
                    if !aborted() {
                        let _ = failed.try_set(e.message);
                    }
                }
            }
        });
    }

    // Focus reader root on mount.
    Effect::new(move |_| {
        if let Some(node) = root.get() {
            let opts = web_sys::FocusOptions::new();
            opts.set_prevent_scroll(true);
            let _ = node.focus_with_options(&opts);
        }
    });

    // Preference signals.
    let theme = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksTheme"))
                .unwrap_or("dar")
                .to_owned()
        })
    });
    let font = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksFont"))
                .unwrap_or("serif")
                .to_owned()
        })
    });
    let font_size = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_u32(p, "booksFontSize"))
                .unwrap_or(18)
        })
    });
    let line_height_pref = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksLineHeight"))
                .unwrap_or("normal")
                .to_owned()
        })
    });
    let layout = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksLayout"))
                .unwrap_or("pages")
                .to_owned()
        })
    });
    let comic_direction = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksComicDirection"))
                .unwrap_or("auto")
                .to_owned()
        })
    });

    // Compute ReaderLook from preferences & stage styles (Dar theme reads computed styles).
    Effect::new(move |_| {
        let _has_detail = detail.with(Option::is_some);
        let t = theme.get();
        let f = font.get();
        let sz = font_size.get();
        let lh = line_height_value(&line_height_pref.get());
        let ly = layout.get();
        let Some(node) = stage.get() else { return };
        let (bg, color, link, dark) = if t == "dar" {
            let css = window().get_computed_style(&node).ok().flatten();
            let bg = css
                .as_ref()
                .and_then(|c| c.get_property_value("background-color").ok())
                .unwrap_or_else(|| "#ffffff".into());
            let color = css
                .as_ref()
                .and_then(|c| c.get_property_value("color").ok())
                .unwrap_or_else(|| "#1f1a17".into());
            let link = node
                .query_selector(".books-accent-probe")
                .ok()
                .flatten()
                .and_then(|p| window().get_computed_style(&p).ok().flatten())
                .and_then(|c| c.get_property_value("color").ok())
                .unwrap_or_else(|| color.clone());
            let dark = document()
                .document_element()
                .and_then(|e| e.get_attribute("data-theme"))
                .as_deref()
                == Some("mocha");
            (bg, color, link, dark)
        } else {
            let (b, c, l, d) = fixed_theme(&t);
            (b.into(), c.into(), l.into(), d)
        };
        look.set(Some(ReaderLook {
            background: bg,
            color,
            link,
            dark,
            font: f,
            size: sz,
            line_height: lh,
            layout: ly,
        }));
    });

    let look_ready = Memo::new(move |_| look.with(Option::is_some));
    let detail_ready = Memo::new(move |_| detail.with(Option::is_some));
    let has_failed = Memo::new(move |_| !failed.with(String::is_empty));

    // Flush pending progress to the server; 409 checks whether another device is elsewhere.
    let flush = {
        let alive = alive.clone();
        move || {
            let Some(next) = pending.try_update_value(|slot| slot.take()).flatten() else {
                return;
            };
            let rev = revision.try_get_value().unwrap_or(0);
            let cur_status = status.try_get_value().unwrap_or_default();
            let Some(bid) = book_sv.try_with_value(|b| b.id.clone()) else {
                return;
            };
            let Some(csrf) = csrf_sv.try_get_value() else {
                return;
            };
            let alive = alive.clone();
            spawn_local(async move {
                let put_once = |r: u64, st_cur: String, w: Where| {
                    let bid = bid.clone();
                    let csrf = csrf.clone();
                    async move {
                        let finished = w.progress >= 0.999;
                        let st_next = if finished || st_cur == "finished" {
                            "finished"
                        } else {
                            "reading"
                        };
                        let body = json!({
                            "revision": r,
                            "position": w.position,
                            "progress": w.progress,
                            "status": st_next,
                            "device": device_name(),
                        });
                        api::send::<BookState>(
                            &format!("{API}/{bid}/state"),
                            "PUT",
                            Some(body),
                            &csrf,
                        )
                        .await
                    }
                };
                match put_once(rev, cur_status, next.clone()).await {
                    Ok(st) => {
                        if alive.load(Ordering::Relaxed) {
                            let _ = revision.try_set_value(st.revision);
                            let _ = status.try_set_value(st.status.clone());
                            let _ = save_error.try_set(String::new());
                        }
                        {
                            on_state.try_run(st);
                        };
                    }
                    Err(e) if e.status == 409 => {
                        if !alive.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(current) = api::get::<BookDetail>(&format!("{API}/{bid}")).await
                            && let Some(st) = current.state
                        {
                            let _ = revision.try_set_value(st.revision);
                            let _ = status.try_set_value(st.status.clone());
                            if st.position.is_some() && (st.progress - next.progress).abs() > 0.005
                            {
                                let _ = conflict.try_set(Some(Conflict {
                                    state: st,
                                    position: Some(next.position),
                                }));
                            } else if let Ok(updated) =
                                put_once(st.revision, st.status.clone(), next).await
                            {
                                if alive.load(Ordering::Relaxed) {
                                    let _ = revision.try_set_value(updated.revision);
                                    let _ = status.try_set_value(updated.status.clone());
                                    let _ = save_error.try_set(String::new());
                                }
                                {
                                    on_state.try_run(updated);
                                };
                            }
                        }
                    }
                    Err(_) => {
                        if alive.load(Ordering::Relaxed) {
                            let _ = save_error.try_set(
                                "Your place could not be saved. It will be saved with your next page turn.".into(),
                            );
                        }
                    }
                }
            });
        }
    };

    let moved = {
        let flush = flush.clone();
        move |position: BookPosition, progress: f64| {
            let next = Where { position, progress };
            current_where.set(Some(next.clone()));
            if conflict.with_untracked(Option::is_some) {
                return;
            }
            pending.set_value(Some(next));
            let seq = save_seq.get_value() + 1;
            save_seq.set_value(seq);
            let flush = flush.clone();
            spawn_local(async move {
                crate::sleep(1200).await;
                if save_seq.try_get_value() == Some(seq) {
                    flush();
                }
            });
        }
    };

    // Flush on unmount and when the tab hides.
    {
        let flush_vis = flush.clone();
        listen(&document(), "visibilitychange", false, move |_| {
            let hidden = Reflect::get(&document(), &JsValue::from_str("visibilityState"))
                .ok()
                .and_then(|v| v.as_string())
                .as_deref()
                == Some("hidden");
            if hidden {
                flush_vis();
            }
        });
    }
    {
        let flush_drop = flush.clone();
        let alive_drop = alive.clone();
        on_cleanup(move || {
            flush_drop();
            alive_drop.store(false, Ordering::Relaxed);
        });
    }

    let chapter_label = move |pos: Option<&BookPosition>| -> String {
        let Some(pos) = pos else {
            return String::new();
        };
        if let Some(spine) = pos.spine {
            let mut label = String::new();
            detail.with(|d| {
                if let Some(BookDetail {
                    manifest: BookManifest::Epub { toc, .. },
                    ..
                }) = d
                {
                    for entry in toc {
                        if entry.spine >= 0 && (entry.spine as usize) <= spine {
                            label = entry.label.clone();
                        }
                    }
                }
            });
            return label;
        }
        let mut label = String::new();
        let page = pos.page.unwrap_or(0);
        pdf_info.with(|p| {
            if let Some(info) = p {
                for entry in &info.outline {
                    if entry.page >= 0 && (entry.page as usize) <= page {
                        label = entry.label.clone();
                    }
                }
            }
        });
        label
    };

    let right_to_left = Memo::new(move |_| {
        let dir = comic_direction.get();
        detail.with(|d| match d.as_ref().map(|d| &d.manifest) {
            Some(BookManifest::Comic { right_to_left, .. }) => {
                dir == "rtl" || (dir == "auto" && *right_to_left)
            }
            Some(BookManifest::Epub { right_to_left, .. }) => *right_to_left,
            _ => false,
        })
    });

    let go_to = move |pos: BookPosition| {
        if let Some(spine) = pos.spine {
            if let Some(h) = epub_handle.get_untracked() {
                h.go_to(spine, pos.fraction, None);
            }
            return;
        }
        let is_comic_kind = detail.with_untracked(|d| {
            matches!(
                d.as_ref().map(|d| &d.manifest),
                Some(BookManifest::Comic { .. })
            )
        });
        if is_comic_kind {
            if let Some(h) = comic_handle.get_untracked() {
                h.go_to(pos.page.unwrap_or(0));
            }
        } else if let Some(h) = pdf_handle.get_untracked() {
            h.go_to(pos.page.unwrap_or(0), pos.fraction);
        }
    };

    let step = move |direction: i32| {
        let kind = detail.with_untracked(|d| match d.as_ref().map(|d| &d.manifest) {
            Some(BookManifest::Epub { .. }) => "epub",
            Some(BookManifest::Comic { .. }) => "comic",
            _ => "pdf",
        });
        match kind {
            "epub" => {
                if let Some(h) = epub_handle.get_untracked() {
                    if direction > 0 {
                        h.next();
                    } else {
                        h.previous();
                    }
                }
            }
            "comic" => {
                if let Some(h) = comic_handle.get_untracked() {
                    if direction > 0 {
                        h.next();
                    } else {
                        h.previous();
                    }
                }
            }
            _ => {
                if let Some(h) = pdf_handle.get_untracked() {
                    h.step(direction);
                }
            }
        }
    };

    let marked = Memo::new(move |_| {
        let w = current_where.get()?;
        let is_comic_kind = detail.with(|d| {
            matches!(
                d.as_ref().map(|d| &d.manifest),
                Some(BookManifest::Comic { .. })
            )
        });
        marks.with(|list| {
            list.iter()
                .find(|m| {
                    m.position.spine == w.position.spine
                        && m.position.page == w.position.page
                        && (is_comic_kind
                            || (m.position.fraction - w.position.fraction).abs() < 0.02)
                })
                .cloned()
        })
    });

    let toggle_mark = move |_| {
        let Some(w) = current_where.get_untracked() else {
            return;
        };
        let existing = marked.get_untracked();
        let bid = book_sv.with_value(|b| b.id.clone());
        let csrf = csrf_sv.get_value();
        let label = {
            let ch = chapter_label(Some(&w.position));
            if !ch.is_empty() {
                ch
            } else if let Some(p) = w.position.page {
                format!("Page {}", p + 1)
            } else {
                percent(w.progress)
            }
        };
        spawn_local(async move {
            if let Some(m) = existing {
                match api::send::<Value>(
                    &format!("{API}/{bid}/marks/{}", m.id),
                    "DELETE",
                    None,
                    &csrf,
                )
                .await
                {
                    Ok(_) => {
                        let _ = marks.try_update(|list| list.retain(|x| x.id != m.id));
                    }
                    Err(e) => {
                        let _ = save_error.try_set(e.message);
                    }
                }
            } else {
                let body = json!({
                    "id": crate::random_uuid(),
                    "position": w.position,
                    "label": label,
                });
                match api::send::<BookMark>(
                    &format!("{API}/{bid}/marks"),
                    "POST",
                    Some(body),
                    &csrf,
                )
                .await
                {
                    Ok(mark) => {
                        let _ = marks.try_update(|list| list.push(mark));
                    }
                    Err(e) => {
                        let _ = save_error.try_set(e.message);
                    }
                }
            }
        });
    };

    let toggle_fullscreen = move || {
        if is_fullscreen() {
            call_promise_method(&document(), "exitFullscreen");
        } else if let Some(node) = root.get_untracked() {
            call_promise_method(&node, "requestFullscreen");
        }
    };

    let on_keydown = move |event: web_sys::KeyboardEvent| {
        if event.default_prevented() {
            return;
        }
        let pdf_open = detail
            .with_untracked(|d| matches!(d.as_ref().map(|d| &d.manifest), Some(BookManifest::Pdf)));
        if pdf_open
            && (event.ctrl_key() || event.meta_key())
            && !event.alt_key()
            && event.key().eq_ignore_ascii_case("f")
        {
            event.prevent_default();
            open_find();
            return;
        }
        let blocked = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| {
                el.closest("input,select,textarea,[role=menu],[role=dialog],.books-reader-panel,.books-typography")
                    .ok()
                    .flatten()
            })
            .is_some();
        if blocked {
            return;
        }
        let key = event.key();
        let is_pdf = detail
            .with_untracked(|d| matches!(d.as_ref().map(|d| &d.manifest), Some(BookManifest::Pdf)));
        let rtl = right_to_left.get_untracked();
        if key == "Escape" {
            event.prevent_default();
            if typography.get_untracked() {
                typography.set(false);
            } else if is_fullscreen() {
                call_promise_method(&document(), "exitFullscreen");
            } else {
                {
                    on_close.try_run(());
                };
            }
        } else if key == (if rtl { "ArrowLeft" } else { "ArrowRight" })
            || (!is_pdf && (key == "PageDown" || (key == " " && !event.shift_key())))
        {
            event.prevent_default();
            step(1);
        } else if key == (if rtl { "ArrowRight" } else { "ArrowLeft" })
            || (!is_pdf && (key == "PageUp" || (key == " " && event.shift_key())))
        {
            event.prevent_default();
            step(-1);
        } else if key == "f" && !event.ctrl_key() && !event.meta_key() {
            event.prevent_default();
            toggle_fullscreen();
        }
    };

    let progress_value = Memo::new(move |_| {
        current_where
            .with(|w| w.as_ref().map(|w| w.progress))
            .unwrap_or_else(|| book_sv.with_value(|b| b.state.as_ref().map_or(0.0, |s| s.progress)))
    });

    let total_pages = Memo::new(move |_| {
        detail.with(|d| match d.as_ref().map(|d| &d.manifest) {
            Some(BookManifest::Comic { pages, .. }) => *pages,
            _ => pdf_info
                .with(|p| p.as_ref().map(|p| p.pages))
                .unwrap_or_else(|| book_sv.with_value(|b| b.pages)),
        })
    });

    let footer_text = Memo::new(move |_| {
        let Some(w) = current_where.get() else {
            return String::new();
        };
        if let Some(p) = w.position.page {
            let pages = total_pages.get();
            if pages > 0 {
                format!("Page {} of {pages}", p + 1)
            } else {
                format!("Page {} of \u{2026}", p + 1)
            }
        } else {
            chapter_label(Some(&w.position))
        }
    });

    let set_pref = move |key: &str, val: Value| {
        let mut map = Map::new();
        map.insert(key.into(), val);
        {
            on_preferences.try_run(map);
        };
    };

    let on_scrub = move |ev: web_sys::Event| {
        let val = event_target_value(&ev).parse::<f64>().unwrap_or(0.0) / 1000.0;
        detail.with_untracked(|d| match d.as_ref().map(|d| &d.manifest) {
            Some(BookManifest::Epub { spine, .. }) => {
                let sizes: Vec<f64> = spine.iter().map(|s| s.size).collect();
                let sum: f64 = sizes.iter().sum();
                let total = if sum > 0.0 { sum } else { 1.0 };
                let mut at = 0.0;
                let mut index = 0usize;
                while index + 1 < sizes.len() && at + sizes[index] < val * total {
                    at += sizes[index];
                    index += 1;
                }
                let denom = sizes
                    .get(index)
                    .copied()
                    .filter(|&s| s > 0.0)
                    .unwrap_or(1.0);
                let frac = ((val * total - at) / denom).clamp(0.0, 1.0);
                if let Some(h) = epub_handle.get_untracked() {
                    h.go_to(index, frac, None);
                }
            }
            Some(BookManifest::Comic { pages, .. }) => {
                if let Some(h) = comic_handle.get_untracked() {
                    let target = (val * (pages.saturating_sub(1) as f64)).round() as usize;
                    h.go_to(target);
                }
            }
            Some(BookManifest::Pdf) => {
                if let Some(info) = pdf_info.get_untracked()
                    && let Some(h) = pdf_handle.get_untracked()
                {
                    let target = (val * (info.pages.saturating_sub(1) as f64)).round() as usize;
                    h.go_to(target, 0.0);
                }
            }
            None => {}
        });
    };

    let font_select_val = RwSignal::new(font.get_untracked());
    Effect::new(move |_| font_select_val.set(font.get()));

    let book_title = book.title.clone();
    let reader_aria = format!("Reading {}", book.title);

    view! {
        <main
            node_ref=root
            class=move || format!(
                "pim-main books-reader books-theme-{}{}",
                theme.get(),
                if panel.with(Option::is_some) { " has-panel" } else { "" }
            )
            tabindex="-1"
            aria-label=reader_aria
            on:keydown=on_keydown
        >
            <div class="books-reader-bar">
                <button class="icon-button" aria-label="Back to library" title="Back to library (Escape)" on:click=move |_| { on_close.try_run(()); }>
                    <Icon name="back"/>
                </button>
                <h1>{book_title.clone()}</h1>
                {move || {
                    let ch = current_where.with(|w| chapter_label(w.as_ref().map(|w| &w.position)));
                    (!ch.is_empty()).then(|| view! {
                        <span class="books-reader-chapter"><Icon name="chevron"/>{ch}</span>
                    })
                }}
                <div class="books-tools">
                    {move || {
                        let is_pdf = detail.with(|d| matches!(d.as_ref().map(|d| &d.manifest), Some(BookManifest::Pdf)));
                        is_pdf.then(|| view! {
                            <button
                                class=move || if find_open.get() { "icon-button is-active" } else { "icon-button" }
                                aria-label="Find in book"
                                title="Find in book (Ctrl+F)"
                                aria-expanded=move || find_open.get().to_string()
                                on:click=move |_| if find_open.get_untracked() { close_find() } else { open_find() }
                            >
                                <Icon name="search"/>
                            </button>
                            <span class="books-zoom" role="group" aria-label="Zoom">
                                <button class="icon-button" aria-label="Zoom out" on:click=move |_| { if let Some(h) = pdf_handle.get_untracked() { h.zoom(PdfZoomChange::Out); } }><Icon name="minus"/></button>
                                <button class="icon-button" aria-label="Fit width" title="Fit width" on:click=move |_| { if let Some(h) = pdf_handle.get_untracked() { h.zoom(PdfZoomChange::Width); } }><Icon name="fitWidth"/></button>
                                <button class="icon-button" aria-label="Zoom in" on:click=move |_| { if let Some(h) = pdf_handle.get_untracked() { h.zoom(PdfZoomChange::In); } }><Icon name="plus"/></button>
                            </span>
                        })
                    }}
                    {move || {
                        let is_epub = detail.with(|d| matches!(d.as_ref().map(|d| &d.manifest), Some(BookManifest::Epub { .. })));
                        is_epub.then(|| view! {
                            <button
                                node_ref=aa
                                class=move || if typography.get() { "icon-button is-active" } else { "icon-button" }
                                aria-label="Text and layout"
                                title="Text and layout"
                                aria-expanded=move || typography.get().to_string()
                                on:click=move |_| typography.update(|v| *v = !*v)
                            >
                                <Icon name="typography"/>
                            </button>
                        })
                    }}
                    <button
                        class=move || if marked.with(Option::is_some) { "icon-button is-on" } else { "icon-button" }
                        aria-pressed=move || marked.with(Option::is_some).to_string()
                        aria-label=move || if marked.with(Option::is_some) { "Remove bookmark" } else { "Add bookmark" }
                        title=move || if marked.with(Option::is_some) { "Remove bookmark" } else { "Bookmark this page" }
                        disabled=move || current_where.with(Option::is_none)
                        on:click=toggle_mark
                    >
                        <Icon name="bookmark"/>
                    </button>
                    <button
                        class=move || if panel.with(Option::is_some) { "icon-button is-active" } else { "icon-button" }
                        aria-pressed=move || panel.with(Option::is_some).to_string()
                        aria-label="Contents and bookmarks"
                        title="Contents and bookmarks"
                        on:click=move |_| panel.update(|p| *p = if p.is_some() { None } else { Some("contents") })
                    >
                        <Icon name="panel"/>
                    </button>
                    <button
                        class="icon-button books-fullscreen"
                        aria-label="Full screen"
                        title="Full screen (F)"
                        on:click=move |_| toggle_fullscreen()
                    >
                        <Icon name="expand"/>
                    </button>
                    <ActionMenu label="More reading actions" actions=actions>
                        <Icon name="more"/>
                    </ActionMenu>
                </div>
            </div>

            <Show when=move || find_open.get()>
                <div class="books-find" role="search" aria-label="Find in book">
                    <Icon name="search"/>
                    <input
                        node_ref=find_input
                        type="search"
                        aria-label="Find in book"
                        placeholder="Find in book"
                        autocomplete="off"
                        prop:value=move || find_text.get()
                        on:input=move |ev| {
                            let value = event_target_value(&ev);
                            find_text.set(value.clone());
                            if let Some(h) = pdf_handle.get_untracked() {
                                h.find(&value);
                            }
                        }
                        on:keydown=move |event: web_sys::KeyboardEvent| {
                            match event.key().as_str() {
                                "Enter" => {
                                    event.prevent_default();
                                    if let Some(h) = pdf_handle.get_untracked() {
                                        h.find_step(if event.shift_key() { -1 } else { 1 });
                                    }
                                }
                                "Escape" => {
                                    event.prevent_default();
                                    event.stop_propagation();
                                    close_find();
                                    if let Some(node) = root.get_untracked() {
                                        let _ = node.focus();
                                    }
                                }
                                _ => {}
                            }
                        }
                    />
                    <span class="books-find-count" role="status" aria-live="polite">
                        {move || {
                            let state = find_state.get();
                            if find_text.with(|t| t.trim().is_empty()) {
                                String::new()
                            } else if state.matches == 0 {
                                if state.complete { "No matches".into() } else { "Searching\u{2026}".into() }
                            } else {
                                let more = if state.complete { "" } else { "+" };
                                format!("{} of {}{more}", state.current.unwrap_or(0), state.matches)
                            }
                        }}
                    </span>
                    <button
                        class="icon-button"
                        aria-label="Previous match"
                        title="Previous match (Shift+Enter)"
                        disabled=move || find_state.with(|s| s.matches == 0)
                        on:click=move |_| { if let Some(h) = pdf_handle.get_untracked() { h.find_step(-1); } }
                    >
                        <span class="books-find-prev"><Icon name="chevron"/></span>
                    </button>
                    <button
                        class="icon-button"
                        aria-label="Next match"
                        title="Next match (Enter)"
                        disabled=move || find_state.with(|s| s.matches == 0)
                        on:click=move |_| { if let Some(h) = pdf_handle.get_untracked() { h.find_step(1); } }
                    >
                        <span class="books-find-next"><Icon name="chevron"/></span>
                    </button>
                    <button class="icon-button" aria-label="Close find" title="Close (Escape)" on:click=move |_| close_find()>
                        <Icon name="close"/>
                    </button>
                </div>
            </Show>

            <Show when=move || typography.get() && look_ready.get()>
                <div
                    node_ref=aa_panel
                    class="books-typography"
                    role="dialog"
                    aria-label="Text and layout"
                    on:keydown=move |event| {
                        if event.key() == "Escape" {
                            event.prevent_default();
                            event.stop_propagation();
                            typography.set(false);
                            if let Some(btn) = aa.get_untracked() {
                                let _ = btn.focus();
                            }
                        }
                    }
                >
                    <fieldset>
                        <legend>"Theme"</legend>
                        <div class="books-themes">
                            {[("dar", "Dar"), ("paper", "Paper"), ("sepia", "Sepia"), ("night", "Night")].into_iter().map(|(val, label)| view! {
                                <button
                                    type="button"
                                    class=format!("books-theme-swatch is-{val}")
                                    aria-pressed=move || (theme.get() == val).to_string()
                                    on:click=move |_| set_pref("booksTheme", json!(val))
                                >
                                    <i>"Aa"</i>{label}
                                </button>
                            }).collect_view()}
                        </div>
                    </fieldset>
                    <label>
                        "Font"
                        <Select
                            options=vec![
                                ("serif".into(), "Serif".into()),
                                ("sans".into(), "Sans serif".into()),
                                ("book".into(), "The book\u{2019}s own".into()),
                            ]
                            value=font_select_val
                            on_change=Callback::new(move |v: String| set_pref("booksFont", json!(v)))
                        />
                    </label>
                    <div class="books-size">
                        <span>"Size "<small>{move || format!("{} px", font_size.get())}</small></span>
                        <div>
                            <button
                                class="icon-button"
                                aria-label="Smaller text"
                                disabled={move || font_size.get() <= FONT_SIZES[0]}
                                on:click=move |_| {
                                    let cur = font_size.get_untracked();
                                    let idx = FONT_SIZES.iter().position(|&n| n == cur).unwrap_or(4);
                                    let next = FONT_SIZES[idx.saturating_sub(1)];
                                    set_pref("booksFontSize", json!(next));
                                }
                            ><Icon name="minus"/></button>
                            <button
                                class="icon-button"
                                aria-label="Larger text"
                                disabled={move || font_size.get() >= *FONT_SIZES.last().unwrap()}
                                on:click=move |_| {
                                    let cur = font_size.get_untracked();
                                    let idx = FONT_SIZES.iter().position(|&n| n == cur).unwrap_or(4);
                                    let next = FONT_SIZES[(idx + 1).min(FONT_SIZES.len() - 1)];
                                    set_pref("booksFontSize", json!(next));
                                }
                            ><Icon name="plus"/></button>
                        </div>
                    </div>
                    <span class="books-typography-label">"Line spacing"</span>
                    <Segmented
                        label="Line spacing"
                        value=Signal::derive(move || line_height_pref.get())
                        options=vec![("tight", "Tight"), ("normal", "Normal"), ("loose", "Loose")]
                        on_change=Callback::new(move |v: String| set_pref("booksLineHeight", json!(v)))
                    />
                    <span class="books-typography-label">"Layout"</span>
                    <Segmented
                        label="Layout"
                        value=Signal::derive(move || layout.get())
                        options=vec![("pages", "Pages"), ("scroll", "Scroll")]
                        on_change=Callback::new(move |v: String| set_pref("booksLayout", json!(v)))
                    />
                    <p class="books-hint">"Saved to your account for every book."</p>
                </div>
            </Show>

            <div class="books-reader-body">
                <div node_ref=stage class="books-stage">
                    <span class="books-accent-probe" aria-hidden="true"></span>
                    {move || {
                        if has_failed.get() {
                            let msg = failed.get();
                            return view! {
                                <div class="books-empty" role="alert">
                                    <h2>"This book could not be opened"</h2>
                                    <p>{msg}</p>
                                    <button class="secondary-button" on:click=move |_| { on_close.try_run(()); }>"Back to library"</button>
                                </div>
                            }.into_any();
                        }
                        if !detail_ready.get() || !look_ready.get() {
                            return view! {
                                <p class="books-empty" role="status">
                                    {book_sv.with_value(|b| format!("Opening {}\u{2026}", b.title))}
                                </p>
                            }.into_any();
                        }
                        let Some(det) = detail.get_untracked() else {
                            return ().into_any();
                        };
                        let start = det.state.as_ref().and_then(|s| s.position.clone());
                        let b = book_sv.get_value();
                        let moved_c = moved.clone();
                        match det.manifest {
                            BookManifest::Epub { spine, .. } => {
                                let spine_sizes: Vec<f64> = spine.iter().map(|s| s.size).collect();
                                let sum: f64 = spine_sizes.iter().sum();
                                let spine_total = if sum > 0.0 { sum } else { 1.0 };
                                let spine_for_links = spine.clone();
                                let bid_for_links = b.id.clone();
                                let start_spine = start.as_ref().and_then(|p| p.spine).unwrap_or(0);
                                let start_fraction = start.as_ref().map_or(0.0, |p| p.fraction);
                                view! {
                                    <EpubView
                                        base=format!("{API}/{}/epub/r{}/", b.id, b.revision)
                                        spine=spine
                                        right_to_left=Signal::from(right_to_left)
                                        look=Signal::derive(move || look.get().unwrap_or_else(default_look))
                                        start_spine=start_spine
                                        start_fraction=start_fraction
                                        label=b.title
                                        handle=epub_handle
                                        on_error=Callback::new(move |msg: String| failed.set(msg))
                                        on_link=Callback::new(move |url: String| {
                                            let base_href = window().location().href().unwrap_or_default();
                                            let Ok(target) = web_sys::Url::new_with_base(&url, &base_href) else {
                                                return;
                                            };
                                            let same_origin = window().location().origin().ok().as_deref() == Some(target.origin().as_str());
                                            let prefix = format!("{API}/{bid_for_links}/epub/");
                                            if same_origin && target.pathname().starts_with(&prefix) {
                                                let joined = target.pathname().split('/').skip(7).collect::<Vec<_>>().join("/");
                                                let path = js_sys::decode_uri_component(&joined)
                                                    .ok()
                                                    .and_then(|v| v.as_string())
                                                    .unwrap_or(joined);
                                                if let Some(idx) = spine_for_links.iter().position(|s| s.href == path) {
                                                    let hash = target.hash();
                                                    let anchor = hash.strip_prefix('#').filter(|s| !s.is_empty()).map(str::to_owned);
                                                    if let Some(h) = epub_handle.get_untracked() {
                                                        h.go_to(idx, 0.0, anchor);
                                                    }
                                                }
                                            } else {
                                                let _ = window().open_with_url_and_target_and_features(
                                                    &target.href(),
                                                    "_blank",
                                                    "noopener,noreferrer",
                                                );
                                            }
                                        })
                                        on_position=Callback::new(move |(sp, frac): (usize, f64)| {
                                            let before: f64 = spine_sizes.iter().take(sp).sum();
                                            let cur_sz = spine_sizes.get(sp).copied().unwrap_or(0.0);
                                            let prog = ((before + frac * cur_sz) / spine_total).min(1.0);
                                            moved_c(BookPosition { spine: Some(sp), page: None, fraction: frac }, prog);
                                        })
                                    />
                                }.into_any()
                            }
                            BookManifest::Comic { pages, .. } => {
                                let start_page = start_page.unwrap_or_else(|| start.as_ref().and_then(|p| p.page).unwrap_or(0));
                                view! {
                                    <ComicView
                                        base=format!("{API}/{}/pages/", b.id)
                                        pages=pages
                                        start=start_page
                                        right_to_left=Signal::from(right_to_left)
                                        label=b.title
                                        handle=comic_handle
                                        on_page=Callback::new(move |page: usize| {
                                            let prog = if pages > 1 { page as f64 / (pages - 1) as f64 } else { 1.0 };
                                            moved_c(BookPosition { spine: None, page: Some(page), fraction: 0.0 }, prog);
                                        })
                                    />
                                }.into_any()
                            }
                            BookManifest::Pdf => {
                                let (start_page, start_fraction) = match start_page {
                                    Some(page) => (page, 0.0),
                                    None => (
                                        start.as_ref().and_then(|p| p.page).unwrap_or(0),
                                        start.as_ref().map_or(0.0, |p| p.fraction),
                                    ),
                                };
                                let (hl_book, hl_title) = (b.id.clone(), b.title.clone());
                                view! {
                                    <PdfView
                                        url=format!("/api/v1/files/{}/content?v={}", b.id, b.revision)
                                        page=start_page
                                        fraction=start_fraction
                                        label=b.title
                                        handle=pdf_handle
                                        highlights=Signal::from(highlights)
                                        on_find=Callback::new(move |state: PdfFindState| find_state.set(state))
                                        on_highlight=Callback::new(move |selection: PdfSelection| {
                                            let (bid, title) = (hl_book.clone(), hl_title.clone());
                                            let session = session_sv.get_value();
                                            spawn_local(async move {
                                                match highlights::save(&session, &bid, &title, &selection).await {
                                                    Ok(saved) => {
                                                        let _ = highlights.try_update(|list| list.push(saved));
                                                        show_status(format!("Highlight saved to Notes \u{203a} {} \u{203a} {}", highlights::FOLDER, highlights::note_name(&title)));
                                                    }
                                                    Err(message) => {
                                                        let _ = save_error.try_set(message);
                                                    }
                                                }
                                            });
                                        })
                                        on_ready=Callback::new(move |info: PdfReady| pdf_info.set(Some(info)))
                                        on_error=Callback::new(move |msg: String| failed.set(msg))
                                        on_position=Callback::new(move |(page, frac, count): (usize, f64, usize)| {
                                            let prog = if count > 1 {
                                                ((page as f64 + frac) / (count - 1) as f64).min(1.0)
                                            } else {
                                                1.0
                                            };
                                            moved_c(BookPosition { spine: None, page: Some(page), fraction: frac }, prog);
                                        })
                                    />
                                }.into_any()
                            }
                        }
                    }}
                </div>

                {move || {
                    let active_tab = panel.get()?;
                    Some(view! {
                        <aside class="books-reader-panel" aria-label="Contents and bookmarks">
                            <div class="books-panel-tabs" role="tablist">
                                <button
                                    role="tab"
                                    aria-selected=(active_tab == "contents").to_string()
                                    on:click=move |_| panel.set(Some("contents"))
                                >"Contents"</button>
                                <button
                                    role="tab"
                                    aria-selected=(active_tab == "bookmarks").to_string()
                                    on:click=move |_| panel.set(Some("bookmarks"))
                                >
                                    "Bookmarks"
                                    {move || {
                                        let n = marks.with(Vec::len);
                                        (n > 0).then(|| view! { <small>{n}</small> })
                                    }}
                                </button>
                                <button class="icon-button" aria-label="Close contents" on:click=move |_| panel.set(None)>
                                    <Icon name="close"/>
                                </button>
                            </div>
                            <div class="books-panel-body" role="tabpanel">
                                {move || {
                                    if active_tab == "contents" {
                                        let toc_opt: Option<Vec<BookTocEntry>> = detail.with(|d| match d.as_ref().map(|d| &d.manifest) {
                                            Some(BookManifest::Epub { toc, .. }) => Some(toc.clone()),
                                            _ => None,
                                        });
                                        if let Some(toc) = toc_opt {
                                            if toc.is_empty() {
                                                return view! { <p class="books-hint">"This book has no table of contents."</p> }.into_any();
                                            }
                                            let cur_pos = current_where.with(|w| w.as_ref().map(|w| w.position.clone()));
                                            let cur_ch = chapter_label(cur_pos.as_ref());
                                            return toc.into_iter().map(|entry| {
                                                let is_cur = cur_pos.as_ref().and_then(|p| p.spine) == Some(entry.spine as usize)
                                                    && entry.spine >= 0
                                                    && cur_ch == entry.label;
                                                let anchor = entry.href.split_once('#').map(|(_, a)| a.to_owned());
                                                let spine_idx = entry.spine;
                                                view! {
                                                    <button
                                                        type="button"
                                                        class=format!("books-toc-row depth-{}{}", entry.depth, if is_cur { " is-current" } else { "" })
                                                        disabled=spine_idx < 0
                                                        on:click=move |_| {
                                                            if spine_idx >= 0
                                                                && let Some(h) = epub_handle.get_untracked()
                                                            {
                                                                h.go_to(spine_idx as usize, 0.0, anchor.clone());
                                                            }
                                                            if window().match_media("(max-width: 1100px)").ok().flatten().is_some_and(|m| m.matches()) {
                                                                panel.set(None);
                                                            }
                                                        }
                                                    >{entry.label}</button>
                                                }
                                            }).collect_view().into_any();
                                        }
                                        let is_pdf = detail.with(|d| matches!(d.as_ref().map(|d| &d.manifest), Some(BookManifest::Pdf)));
                                        if is_pdf {
                                            let outline: Vec<PdfOutlineEntry> = pdf_info.with(|p| p.as_ref().map(|p| p.outline.clone()).unwrap_or_default());
                                            if !outline.is_empty() {
                                                return outline.into_iter().map(|entry| {
                                                    let page_idx = entry.page;
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class=format!("books-toc-row depth-{}", entry.depth)
                                                            disabled=page_idx < 0
                                                            on:click=move |_| {
                                                                if page_idx >= 0
                                                                    && let Some(h) = pdf_handle.get_untracked()
                                                                {
                                                                    h.go_to(page_idx as usize, 0.0);
                                                                }
                                                                if window().match_media("(max-width: 1100px)").ok().flatten().is_some_and(|m| m.matches()) {
                                                                    panel.set(None);
                                                                }
                                                            }
                                                        >
                                                            <span>{entry.label}</span>
                                                            <small>{page_idx + 1}</small>
                                                        </button>
                                                    }
                                                }).collect_view().into_any();
                                            }
                                        }
                                        let is_comic_kind = detail.with(|d| matches!(d.as_ref().map(|d| &d.manifest), Some(BookManifest::Comic { .. })));
                                        view! {
                                            <p class="books-hint">
                                                {if is_comic_kind {
                                                    "Comics have no table of contents. Use the scrubber below to move through the pages."
                                                } else {
                                                    "This book has no table of contents."
                                                }}
                                            </p>
                                        }.into_any()
                                    } else {
                                        let list = marks.get();
                                        if list.is_empty() {
                                            return view! {
                                                <p class="books-hint">"Bookmark a page with the bookmark button. Bookmarks are kept with your account."</p>
                                            }.into_any();
                                        }
                                        list.into_iter().map(|m| {
                                            let pos = m.position.clone();
                                            let mid = m.id.clone();
                                            let mlabel = if m.label.is_empty() { "Bookmark".to_owned() } else { m.label.clone() };
                                            let rm_label = format!("Remove bookmark {}", m.label);
                                            let meta = format!(
                                                "{} \u{00b7} {}",
                                                m.position.page.map(|p| format!("Page {}", p + 1)).unwrap_or_default(),
                                                edited(m.created_at)
                                            );
                                            view! {
                                                <div class="books-mark">
                                                    <button type="button" on:click=move |_| go_to(pos.clone())>
                                                        <Icon name="bookmark"/>
                                                        <span>{mlabel}<small>{meta}</small></span>
                                                    </button>
                                                    <button
                                                        class="icon-button"
                                                        aria-label=rm_label
                                                        on:click=move |_| {
                                                            let mid = mid.clone();
                                                            let bid = book_sv.with_value(|b| b.id.clone());
                                                            let csrf = csrf_sv.get_value();
                                                            spawn_local(async move {
                                                                match api::send::<Value>(&format!("{API}/{bid}/marks/{mid}"), "DELETE", None, &csrf).await {
                                                                    Ok(_) => {
                                                                        let _ = marks.try_update(|list| list.retain(|x| x.id != mid));
                                                                    }
                                                                    Err(e) => {
                                                                        let _ = save_error.try_set(e.message);
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    >
                                                        <Icon name="close"/>
                                                    </button>
                                                </div>
                                            }
                                        }).collect_view().into_any()
                                    }
                                }}
                            </div>
                        </aside>
                    })
                }}
            </div>

            <div class="books-reader-foot">
                <span>{move || footer_text.get()}</span>
                <input
                    type="range"
                    min="0"
                    max="1000"
                    step="1"
                    prop:value=move || ((progress_value.get() * 1000.0).round() as i64).to_string()
                    aria-label="Place in book"
                    aria-valuetext=move || format!("{} read", percent(progress_value.get()))
                    on:input=on_scrub
                    on:change=on_scrub
                />
                <span>{move || percent(progress_value.get())}</span>
            </div>

            {move || {
                let err = save_error.get();
                (!err.is_empty()).then(|| view! { <p class="books-save-error" role="alert">{err}</p> })
            }}

            <p class="books-reader-status" role="status" aria-live="polite">{move || reader_status.get()}</p>

            {move || {
                let c = conflict.get()?;
                let cur_prog = progress_value.get();
                let heading = format!(
                    "{}{}",
                    if c.state.progress > cur_prog { "You read further" } else { "Your place changed" },
                    if !c.state.device.is_empty() {
                        format!(" on {}", c.state.device)
                    } else {
                        " on another device".into()
                    }
                );
                let there_page = c
                    .state
                    .position
                    .as_ref()
                    .and_then(|p| p.page)
                    .map(|p| format!(", page {}", p + 1))
                    .unwrap_or_default();
                let detail_line = format!(
                    "There: {}{there_page} ({}). Here: {}.",
                    percent(c.state.progress),
                    edited(c.state.updated_at),
                    percent(cur_prog),
                );
                let target_pos = c.state.position.clone();
                let flush_stay = flush.clone();
                Some(view! {
                    <div class="books-conflict" role="alertdialog" aria-label="Your place changed on another device">
                        <strong>{heading}</strong>
                        <p>{detail_line}</p>
                        <div class="dialog-actions">
                            <button
                                class="secondary-button"
                                on:click=move |_| {
                                    conflict.set(None);
                                    if let Some(w) = current_where.get_untracked() {
                                        pending.set_value(Some(w));
                                        flush_stay();
                                    }
                                }
                            >"Stay here"</button>
                            <button
                                class="primary-button"
                                autofocus=true
                                on:click=move |_| {
                                    conflict.set(None);
                                    if let Some(pos) = target_pos.clone() {
                                        go_to(pos);
                                    }
                                }
                            >"Go there"</button>
                        </div>
                    </div>
                })
            }}
        </main>
    }
}

#[component]
fn BooksSettings(
    session: BrowserSession,
    library: RwSignal<Option<BookLibrary>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let saver = Saver::new(session);
    let section = RwSignal::new("General");
    let draft: RwSignal<Option<String>> = RwSignal::new(None);

    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || {
            let v = saver.value(key);
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
                .unwrap_or_else(|| default.to_owned())
        })
    };
    let set = move |key: &'static str, value: Value, message: String| {
        saver.change(key, value, message, true);
    };

    let saved_folder = Memo::new(move |_| {
        saver
            .value("booksFolder")
            .as_str()
            .map(str::to_owned)
            .or_else(|| library.with(|l| l.as_ref().map(|l| l.path.clone())))
            .unwrap_or_else(|| DEFAULT_BOOKS_FOLDER.to_owned())
    });
    let folder_value = Memo::new(move |_| draft.get().unwrap_or_else(|| saved_folder.get()));
    let folder_valid = Memo::new(move |_| photo_folder_parts(&folder_value.get()).is_some());

    let size_sig = text("booksFontSize", "18");
    let size_select_val = RwSignal::new(size_sig.get_untracked());
    Effect::new(move |_| size_select_val.set(size_sig.get()));

    view! {
        <SettingsSheet
            title="Books settings"
            icon="book"
            sections=vec!["General", "Reading", "Keyboard shortcuts"]
            section=section
            on_close=on_close
            notice=saver.notice
        >
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "Keyboard shortcuts" => view! {
                    <dl class="ui-shortcuts">
                        <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps"</dd>
                        <dt>"\u{2192} / Space / Page Down"</dt><dd>"Next page (\u{2190} in right-to-left books)"</dd>
                        <dt>"\u{2190} / Shift Space / Page Up"</dt><dd>"Previous page"</dd>
                        <dt>"F"</dt><dd>"Full screen (PDFs and comics)"</dd>
                        <dt>"Escape"</dt><dd>"Close menus, then the book, then details"</dd>
                        <dt>"Tab / Shift Tab"</dt><dd>"Move between controls"</dd>
                    </dl>
                }.into_any(),
                _ if saver.preferences.with(Option::is_none) => view! {
                    <p role="status">"Loading settings\u{2026}"</p>
                }.into_any(),
                "General" => view! {
                    <section class="settings-group" aria-label="Library">
                        <h3>"Library"</h3>
                        <SettingRow
                            label="Books folder"
                            hint=format!("Books finds EPUB, PDF and comic files in Files \u{203a} {}. Its top folders are libraries.", saved_folder.get())
                            state=saver.state("booksFolder")
                            stacked=true
                        >
                            <form
                                class="photos-folder-form"
                                on:submit=move |ev| {
                                    ev.prevent_default();
                                    if let Some(parts) = photo_folder_parts(&folder_value.get_untracked()) {
                                        let joined = parts.join("/");
                                        draft.set(None);
                                        set("booksFolder", json!(joined.clone()), format!("Books now come from {joined}"));
                                    }
                                }
                            >
                                <label>
                                    <span class="sr-only">"Books folder"</span>
                                    <input
                                        prop:value=move || folder_value.get()
                                        maxlength="500"
                                        on:input=move |ev| draft.set(Some(event_target_value(&ev)))
                                        aria-invalid=move || (!folder_valid.get()).to_string()
                                    />
                                </label>
                                <button
                                    class="secondary-button"
                                    disabled=move || !folder_valid.get() || folder_value.get() == saved_folder.get()
                                >"Save"</button>
                            </form>
                            {move || (!folder_valid.get()).then(|| view! {
                                <p class="field-error" role="alert">"Use folder names separated by /, like docs/books."</p>
                            })}
                        </SettingRow>
                        <SettingRow
                            label="Sort books by"
                            hint="Order of the grids"
                            state=saver.state("booksSort")
                            stacked=true
                        >
                            <Segmented
                                label="Sort books by"
                                value=text("booksSort", "recent")
                                options=SORTS.to_vec()
                                on_change=Callback::new(move |next: String| {
                                    let lbl = SORTS.iter().find(|(v, _)| *v == next).map(|(_, l)| *l).unwrap_or("Recently read");
                                    set("booksSort", json!(next), format!("Books sorted by {}", lbl.to_lowercase()));
                                })
                            />
                        </SettingRow>
                    </section>
                }.into_any(),
                _ => {
                    let size_options: Vec<(String, String)> = FONT_SIZES
                        .iter()
                        .map(|n| (n.to_string(), format!("{n} px")))
                        .collect();
                    view! {
                        <section class="settings-group" aria-label="Text">
                            <h3>"Text"</h3>
                            <SettingRow
                                label="Theme"
                                hint="Page colours while reading. Dar follows your appearance setting."
                                state=saver.state("booksTheme")
                                stacked=true
                            >
                                <Segmented
                                    label="Theme"
                                    value=text("booksTheme", "dar")
                                    options=vec![("dar", "Dar"), ("paper", "Paper"), ("sepia", "Sepia"), ("night", "Night")]
                                    on_change=Callback::new(move |next: String| set("booksTheme", json!(next), "Reading theme changed".into()))
                                />
                            </SettingRow>
                            <SettingRow
                                label="Font"
                                hint="Choose the book\u{2019}s own to keep the publisher\u{2019}s fonts"
                                state=saver.state("booksFont")
                                stacked=true
                            >
                                <Segmented
                                    label="Font"
                                    value=text("booksFont", "serif")
                                    options=vec![("serif", "Serif"), ("sans", "Sans serif"), ("book", "The book\u{2019}s own")]
                                    on_change=Callback::new(move |next: String| set("booksFont", json!(next), "Reading font changed".into()))
                                />
                            </SettingRow>
                            <SettingRow label="Text size" hint="" state=saver.state("booksFontSize")>
                                <Select
                                    aria_label="Text size"
                                    options=size_options
                                    value=size_select_val
                                    on_change=Callback::new(move |next: String| {
                                        if let Ok(n) = next.parse::<u32>() {
                                            set("booksFontSize", json!(n), format!("Text size set to {n} px"));
                                        }
                                    })
                                />
                            </SettingRow>
                            <SettingRow label="Line spacing" hint="" state=saver.state("booksLineHeight")>
                                <Segmented
                                    label="Line spacing"
                                    value=text("booksLineHeight", "normal")
                                    options=vec![("tight", "Tight"), ("normal", "Normal"), ("loose", "Loose")]
                                    on_change=Callback::new(move |next: String| set("booksLineHeight", json!(next), "Line spacing changed".into()))
                                />
                            </SettingRow>
                            <SettingRow
                                label="Layout"
                                hint="Turn pages, or scroll through each chapter"
                                state=saver.state("booksLayout")
                            >
                                <Segmented
                                    label="Layout"
                                    value=text("booksLayout", "pages")
                                    options=vec![("pages", "Pages"), ("scroll", "Scroll")]
                                    on_change=Callback::new(move |next: String| {
                                        let label = if next == "pages" { "Pages" } else { "Scroll" };
                                        set("booksLayout", json!(next), format!("Books open in {label} layout"));
                                    })
                                />
                            </SettingRow>
                        </section>
                        <section class="settings-group" aria-label="Comics">
                            <h3>"Comics"</h3>
                            <SettingRow
                                label="Reading direction"
                                hint="Automatic uses the comic\u{2019}s own setting (manga read right to left)"
                                state=saver.state("booksComicDirection")
                                stacked=true
                            >
                                <Segmented
                                    label="Reading direction"
                                    value=text("booksComicDirection", "auto")
                                    options=vec![("auto", "Automatic"), ("ltr", "Left to right"), ("rtl", "Right to left")]
                                    on_change=Callback::new(move |next: String| set("booksComicDirection", json!(next), "Comic direction changed".into()))
                                />
                            </SettingRow>
                        </section>
                    }.into_any()
                }
            }}
        </SettingsSheet>
    }
}
