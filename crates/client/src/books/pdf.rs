//! PDF viewer backed by Dar's `dar-pdf-worker` Web Worker (`hayro` in WebAssembly).
//!
//! Pages are laid out at their real proportions in one scrolling column and only pages
//! near the viewport are drawn on canvases. Reused by both Books and Files preview.
//!
//! One dedicated module worker is created lazily on the first PDF open and shared across
//! all viewers in the page; closing a viewer closes only its document in the worker.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use js_sys::{Array, ArrayBuffer, Function, Object, Reflect, Uint8ClampedArray};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::books::pdf_text::{
    PdfFindState, PdfHighlight, PdfMatch, PdfSelection, PdfTextRun, build_text_layer,
    current_selection, find_on_page, fold_query, paint_marks, parse_runs,
};
use crate::icons::Icon;
use crate::ui::{Listener, document, listen, window};

const GAP: f64 = 12.0;
const PAD: f64 = 16.0;
/// Leave slight headroom below `dar_pdf::MAX_PIXELS` (40_000_000) so rounding never trips it.
const MAX_RENDER_PIXELS: f64 = 39_000_000.0;

// ---- Single-threaded oneshot channel for worker RPCs ----------------------------------------

struct OneshotState<T> {
    value: Option<T>,
    closed: bool,
    waker: Option<Waker>,
}

struct OneshotSender<T>(Rc<RefCell<OneshotState<T>>>);

impl<T> OneshotSender<T> {
    fn send(self, value: T) {
        let waker = {
            let mut state = self.0.borrow_mut();
            state.value = Some(value);
            state.closed = true;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Drop for OneshotSender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut state = self.0.borrow_mut();
            if state.closed {
                return;
            }
            state.closed = true;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

struct OneshotReceiver<T>(Rc<RefCell<OneshotState<T>>>);

impl<T> Future for OneshotReceiver<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.0.borrow_mut();
        if let Some(value) = state.value.take() {
            return Poll::Ready(Some(value));
        }
        if state.closed {
            return Poll::Ready(None);
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let state = Rc::new(RefCell::new(OneshotState {
        value: None,
        closed: false,
        waker: None,
    }));
    (OneshotSender(state.clone()), OneshotReceiver(state))
}

// ---- Shared PDF worker pool -----------------------------------------------------------------

struct OpenedDoc {
    pages: usize,
    sizes: Vec<(f64, f64)>,
    outline: Vec<PdfOutlineEntry>,
}

struct RenderedPage {
    width: u32,
    height: u32,
    buffer: ArrayBuffer,
}

struct SharedWorker {
    worker: web_sys::Worker,
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_error: Closure<dyn FnMut(web_sys::Event)>,
}

impl Drop for SharedWorker {
    fn drop(&mut self) {
        self.worker.set_onmessage(None);
        self.worker.set_onerror(None);
        self.worker.terminate();
    }
}

struct WorkerPool {
    shared: Option<SharedWorker>,
    next_doc_id: u32,
    next_req_id: u32,
    open_waiters: HashMap<u32, OneshotSender<Result<OpenedDoc, String>>>,
    render_waiters: HashMap<u32, OneshotSender<Result<RenderedPage, String>>>,
    text_waiters: HashMap<u32, OneshotSender<Result<Vec<PdfTextRun>, String>>>,
}

impl WorkerPool {
    fn new() -> Self {
        Self {
            shared: None,
            next_doc_id: 1,
            next_req_id: 1,
            open_waiters: HashMap::new(),
            render_waiters: HashMap::new(),
            text_waiters: HashMap::new(),
        }
    }

    fn alloc_doc_id(&mut self) -> u32 {
        let id = self.next_doc_id;
        self.next_doc_id = self.next_doc_id.wrapping_add(1).max(1);
        id
    }

    fn alloc_req_id(&mut self) -> u32 {
        let id = self.next_req_id;
        self.next_req_id = self.next_req_id.wrapping_add(1).max(1);
        id
    }
}

thread_local! {
    static WORKER_POOL: RefCell<WorkerPool> = RefCell::new(WorkerPool::new());
}

fn set_prop(obj: &Object, key: &str, value: &JsValue) {
    let _ = Reflect::set(obj, &JsValue::from_str(key), value);
}

fn get_str(obj: &JsValue, key: &str) -> Option<String> {
    Reflect::get(obj, &JsValue::from_str(key)).ok()?.as_string()
}

fn get_f64(obj: &JsValue, key: &str) -> Option<f64> {
    Reflect::get(obj, &JsValue::from_str(key)).ok()?.as_f64()
}

fn get_u32(obj: &JsValue, key: &str) -> Option<u32> {
    let n = get_f64(obj, key)?;
    if n.is_finite() && n >= 0.0 && n <= u32::MAX as f64 {
        Some(n as u32)
    } else {
        None
    }
}

fn worker_url() -> Result<String, String> {
    document()
        .document_element()
        .and_then(|el| el.get_attribute("data-pdf-worker"))
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "The PDF viewer could not start.".to_owned())
}

fn ensure_worker(pool: &mut WorkerPool) -> Result<web_sys::Worker, String> {
    if let Some(shared) = pool.shared.as_ref() {
        return Ok(shared.worker.clone());
    }
    let url = worker_url()?;
    let opts = web_sys::WorkerOptions::new();
    opts.set_type(web_sys::WorkerType::Module);
    let worker = web_sys::Worker::new_with_options(&url, &opts)
        .map_err(|_| "The PDF viewer could not start.".to_owned())?;
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(|event: web_sys::MessageEvent| {
            handle_worker_message(event.data());
        });
    let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(|_| {
        handle_worker_fatal("The PDF viewer could not start.".to_owned());
    });
    worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    let cloned = worker.clone();
    pool.shared = Some(SharedWorker {
        worker,
        _on_message: on_message,
        _on_error: on_error,
    });
    Ok(cloned)
}

fn handle_worker_fatal(message: String) {
    let (open_waiters, render_waiters, text_waiters) = WORKER_POOL.with(|cell| {
        let mut pool = cell.borrow_mut();
        // Drop detaches handlers and terminates the broken worker before clearing the slot.
        pool.shared.take();
        let opens: Vec<_> = pool.open_waiters.drain().map(|(_, tx)| tx).collect();
        let renders: Vec<_> = pool.render_waiters.drain().map(|(_, tx)| tx).collect();
        let texts: Vec<_> = pool.text_waiters.drain().map(|(_, tx)| tx).collect();
        (opens, renders, texts)
    });
    for tx in text_waiters {
        tx.send(Err(message.clone()));
    }
    for tx in open_waiters {
        tx.send(Err(message.clone()));
    }
    for tx in render_waiters {
        tx.send(Err(message.clone()));
    }
}

fn handle_worker_message(data: JsValue) {
    let Some(kind) = get_str(&data, "type") else {
        return;
    };
    match kind.as_str() {
        "opened" => {
            let Some(id) = get_u32(&data, "id") else {
                return;
            };
            let pages = get_f64(&data, "pages").unwrap_or(0.0).max(0.0) as usize;
            let mut sizes = Vec::with_capacity(pages);
            if let Ok(raw_sizes) = Reflect::get(&data, &JsValue::from_str("sizes"))
                && !raw_sizes.is_null()
                && !raw_sizes.is_undefined()
            {
                let arr = Array::from(&raw_sizes);
                for i in 0..arr.length() {
                    let pair = arr.get(i);
                    let w = Reflect::get(&pair, &JsValue::from_f64(0.0))
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(612.0)
                        .max(1.0);
                    let h = Reflect::get(&pair, &JsValue::from_f64(1.0))
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(792.0)
                        .max(1.0);
                    sizes.push((w, h));
                }
            }
            if sizes.len() < pages {
                sizes.resize(pages, (612.0, 792.0));
            }

            let mut outline = Vec::new();
            if let Ok(raw_outline) = Reflect::get(&data, &JsValue::from_str("outline"))
                && !raw_outline.is_null()
                && !raw_outline.is_undefined()
            {
                let arr = Array::from(&raw_outline);
                for i in 0..arr.length() {
                    if outline.len() >= 2000 {
                        break;
                    }
                    let item = arr.get(i);
                    let title = get_str(&item, "title").unwrap_or_default();
                    let trimmed = title.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let label: String = trimmed.chars().take(200).collect();
                    let page = get_f64(&item, "page").map(|n| n as i32).unwrap_or(-1);
                    let depth = get_f64(&item, "depth")
                        .map(|n| (n as usize).min(6))
                        .unwrap_or(0);
                    outline.push(PdfOutlineEntry { label, page, depth });
                }
            }

            let tx = WORKER_POOL.with(|cell| cell.borrow_mut().open_waiters.remove(&id));
            if let Some(tx) = tx {
                tx.send(Ok(OpenedDoc {
                    pages,
                    sizes,
                    outline,
                }));
            }
        }
        "error" => {
            let Some(id) = get_u32(&data, "id") else {
                return;
            };
            let message = get_str(&data, "message")
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "The PDF could not be loaded.".to_owned());
            let tx = WORKER_POOL.with(|cell| cell.borrow_mut().open_waiters.remove(&id));
            if let Some(tx) = tx {
                tx.send(Err(message));
            }
        }
        "page" => {
            let Some(request) = get_u32(&data, "request") else {
                return;
            };
            let width = get_u32(&data, "width").unwrap_or(0);
            let height = get_u32(&data, "height").unwrap_or(0);
            let buffer = Reflect::get(&data, &JsValue::from_str("buffer"))
                .ok()
                .and_then(|v| v.dyn_into::<ArrayBuffer>().ok());
            let tx = WORKER_POOL.with(|cell| cell.borrow_mut().render_waiters.remove(&request));
            if let Some(tx) = tx {
                match buffer {
                    Some(buffer) if width > 0 && height > 0 => {
                        tx.send(Ok(RenderedPage {
                            width,
                            height,
                            buffer,
                        }));
                    }
                    _ => tx.send(Err("This page could not be drawn.".to_owned())),
                }
            }
        }
        "text" => {
            let Some(request) = get_u32(&data, "request") else {
                return;
            };
            let tx = WORKER_POOL.with(|cell| cell.borrow_mut().text_waiters.remove(&request));
            if let Some(tx) = tx {
                tx.send(Ok(parse_runs(&data)));
            }
        }
        "render-error" => {
            let Some(request) = get_u32(&data, "request") else {
                return;
            };
            let message = get_str(&data, "message")
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "This page could not be drawn.".to_owned());
            if let Some(tx) =
                WORKER_POOL.with(|cell| cell.borrow_mut().text_waiters.remove(&request))
            {
                tx.send(Err(message));
                return;
            }
            let tx = WORKER_POOL.with(|cell| cell.borrow_mut().render_waiters.remove(&request));
            if let Some(tx) = tx {
                tx.send(Err(message));
            }
        }
        "fatal" => {
            let message = get_str(&data, "message")
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "The PDF viewer could not start.".to_owned());
            handle_worker_fatal(message);
        }
        _ => {}
    }
}

fn start_open(url: &str) -> Result<(u32, OneshotReceiver<Result<OpenedDoc, String>>), String> {
    let (tx, rx) = oneshot();
    let (worker, doc_id) = WORKER_POOL.with(|cell| {
        let mut pool = cell.borrow_mut();
        let worker = ensure_worker(&mut pool)?;
        let doc_id = pool.alloc_doc_id();
        pool.open_waiters.insert(doc_id, tx);
        Ok::<_, String>((worker, doc_id))
    })?;

    let msg = Object::new();
    set_prop(&msg, "type", &JsValue::from_str("open"));
    set_prop(&msg, "id", &JsValue::from_f64(doc_id as f64));
    set_prop(&msg, "url", &JsValue::from_str(url));
    if worker.post_message(&msg).is_err() {
        WORKER_POOL.with(|cell| {
            cell.borrow_mut().open_waiters.remove(&doc_id);
        });
        return Err("The PDF could not be loaded.".to_owned());
    }
    Ok((doc_id, rx))
}

fn close_doc(doc_id: u32) {
    let worker = WORKER_POOL.with(|cell| {
        let mut pool = cell.borrow_mut();
        pool.open_waiters.remove(&doc_id);
        pool.shared.as_ref().map(|s| s.worker.clone())
    });
    if let Some(worker) = worker {
        let msg = Object::new();
        set_prop(&msg, "type", &JsValue::from_str("close"));
        set_prop(&msg, "doc", &JsValue::from_f64(doc_id as f64));
        let _ = worker.post_message(&msg);
    }
}

fn start_render(
    doc_id: u32,
    page: usize,
    scale: f64,
) -> Result<(u32, OneshotReceiver<Result<RenderedPage, String>>), String> {
    let (tx, rx) = oneshot();
    let (worker, req_id) = WORKER_POOL.with(|cell| {
        let mut pool = cell.borrow_mut();
        let worker = pool
            .shared
            .as_ref()
            .map(|s| s.worker.clone())
            .ok_or_else(|| "The PDF viewer is not running.".to_owned())?;
        let req_id = pool.alloc_req_id();
        pool.render_waiters.insert(req_id, tx);
        Ok::<_, String>((worker, req_id))
    })?;

    let msg = Object::new();
    set_prop(&msg, "type", &JsValue::from_str("render"));
    set_prop(&msg, "doc", &JsValue::from_f64(doc_id as f64));
    set_prop(&msg, "page", &JsValue::from_f64(page as f64));
    set_prop(&msg, "scale", &JsValue::from_f64(scale));
    set_prop(&msg, "request", &JsValue::from_f64(req_id as f64));
    if worker.post_message(&msg).is_err() {
        WORKER_POOL.with(|cell| {
            cell.borrow_mut().render_waiters.remove(&req_id);
        });
        return Err("This page could not be drawn.".to_owned());
    }
    Ok((req_id, rx))
}

fn cancel_render(req_id: u32) {
    let worker = WORKER_POOL.with(|cell| {
        let mut pool = cell.borrow_mut();
        pool.render_waiters.remove(&req_id);
        pool.text_waiters.remove(&req_id);
        pool.shared.as_ref().map(|s| s.worker.clone())
    });
    if let Some(worker) = worker {
        let msg = Object::new();
        set_prop(&msg, "type", &JsValue::from_str("cancel"));
        set_prop(&msg, "request", &JsValue::from_f64(req_id as f64));
        let _ = worker.post_message(&msg);
    }
}

/// Asks the worker for the text of `page`.
fn start_text(
    doc_id: u32,
    page: usize,
) -> Result<(u32, OneshotReceiver<Result<Vec<PdfTextRun>, String>>), String> {
    let (tx, rx) = oneshot();
    let (worker, req_id) = WORKER_POOL.with(|cell| {
        let mut pool = cell.borrow_mut();
        let worker = pool
            .shared
            .as_ref()
            .map(|s| s.worker.clone())
            .ok_or_else(|| "The PDF viewer is not running.".to_owned())?;
        let req_id = pool.alloc_req_id();
        pool.text_waiters.insert(req_id, tx);
        Ok::<_, String>((worker, req_id))
    })?;
    let msg = Object::new();
    set_prop(&msg, "type", &JsValue::from_str("text"));
    set_prop(&msg, "doc", &JsValue::from_f64(doc_id as f64));
    set_prop(&msg, "page", &JsValue::from_f64(page as f64));
    set_prop(&msg, "request", &JsValue::from_f64(req_id as f64));
    if worker.post_message(&msg).is_err() {
        WORKER_POOL.with(|cell| {
            cell.borrow_mut().text_waiters.remove(&req_id);
        });
        return Err("The page text could not be read.".to_owned());
    }
    Ok((req_id, rx))
}

/// The text of `page`; a failure counts as a page without text.
async fn page_text(doc_id: u32, page: usize) -> Vec<PdfTextRun> {
    match start_text(doc_id, page) {
        Ok((_, rx)) => rx.await.and_then(Result::ok).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Builds a browser `ImageData` directly over a transferred `ArrayBuffer` without copying
/// pixel bytes through the main client's WASM heap.
fn image_data_from_buffer(
    buffer: &ArrayBuffer,
    width: u32,
    height: u32,
) -> Result<web_sys::ImageData, JsValue> {
    let clamped = Uint8ClampedArray::new(buffer);
    let ctor: Function =
        Reflect::get(&js_sys::global(), &JsValue::from_str("ImageData"))?.dyn_into()?;
    let args = Array::of3(
        &clamped,
        &JsValue::from_f64(width as f64),
        &JsValue::from_f64(height as f64),
    );
    Reflect::construct(&ctor, &args)?.dyn_into()
}

// ---- Public API (kept stable for Books workspace and Files preview) -------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct PdfOutlineEntry {
    pub label: String,
    pub page: i32,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdfReady {
    pub pages: usize,
    pub outline: Vec<PdfOutlineEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PdfZoom {
    Width,
    Page,
    Scale(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfZoomChange {
    In,
    Out,
    Width,
    Page,
}

#[derive(Clone)]
pub struct PdfHandle {
    go_to: Callback<(usize, f64)>,
    step: Callback<i32>,
    zoom: Callback<PdfZoomChange>,
    find: Callback<String>,
    find_step: Callback<i32>,
}

impl PdfHandle {
    pub fn go_to(&self, page: usize, fraction: f64) {
        let _ = self.go_to.try_run((page, fraction));
    }
    pub fn step(&self, direction: i32) {
        let _ = self.step.try_run(direction);
    }
    pub fn zoom(&self, change: PdfZoomChange) {
        let _ = self.zoom.try_run(change);
    }
    /// Searches the document's text for `query` (case-insensitive); empty clears the search.
    pub fn find(&self, query: &str) {
        let _ = self.find.try_run(query.to_owned());
    }
    /// Moves to the next (1) or previous (-1) match and scrolls it into view.
    pub fn find_step(&self, direction: i32) {
        let _ = self.find_step.try_run(direction);
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Layout {
    scale: f64,
    tops: Vec<f64>,
    total: f64,
}

fn compute_layout(sizes: &[(f64, f64)], width: f64, height: f64, zoom: PdfZoom) -> Layout {
    let widest = sizes.iter().map(|(pw, _)| *pw).fold(1.0_f64, f64::max);
    let tallest = sizes.iter().map(|(_, ph)| *ph).fold(1.0_f64, f64::max);
    let scale = match zoom {
        PdfZoom::Scale(val) => val,
        PdfZoom::Width => ((width - PAD * 2.0) / widest).max(0.2),
        PdfZoom::Page => {
            (((width - PAD * 2.0) / widest).min((height - PAD * 2.0) / tallest)).max(0.2)
        }
    };
    let mut tops = Vec::with_capacity(sizes.len());
    let mut total = PAD;
    for (_, ph) in sizes {
        tops.push(total);
        total += *ph * scale + GAP;
    }
    Layout { scale, tops, total }
}

/// A PDF drawn by Dar's `dar-pdf-worker`: pages are laid out at their real proportions in one
/// scrolling column, and only pages near the viewport are drawn on canvases.
#[component]
pub fn PdfView(
    #[prop(into)] url: Signal<String>,
    #[prop(optional)] page: usize,
    #[prop(optional)] fraction: f64,
    #[prop(optional, into)] class_name: String,
    #[prop(into)] label: Signal<String>,
    #[prop(optional)] handle: Option<RwSignal<Option<PdfHandle>>>,
    #[prop(optional)] on_ready: Option<Callback<PdfReady>>,
    #[prop(optional)] on_position: Option<Callback<(usize, f64, usize)>>,
    #[prop(optional)] on_zoom: Option<Callback<PdfZoom>>,
    #[prop(optional)] on_error: Option<Callback<String>>,
    /// Saved highlights to draw over their pages.
    #[prop(optional)]
    highlights: Option<Signal<Vec<PdfHighlight>>>,
    /// When set, selecting text offers a Highlight action that reports the selection here.
    #[prop(optional)]
    on_highlight: Option<Callback<PdfSelection>>,
    /// Find's progress after each search or step (see [`PdfHandle::find`]).
    #[prop(optional)]
    on_find: Option<Callback<PdfFindState>>,
    /// Reports the effective zoom scale (1.0 = 100%) whenever layout recalculates.
    #[prop(optional)]
    on_scale: Option<Callback<f64>>,
    /// Paper tint mode ("paper", "sepia", "night").
    #[prop(optional)]
    tint: Option<Signal<String>>,
) -> impl IntoView {
    let scroller = NodeRef::<html::Div>::new();
    let pages_node = NodeRef::<html::Div>::new();
    // Each page's text, read once per document when the page is first drawn or searched.
    let texts: RwSignal<HashMap<usize, Arc<Vec<PdfTextRun>>>> = RwSignal::new(HashMap::new());
    let text_pending: StoredValue<HashSet<usize>> = StoredValue::new(HashSet::new());
    let find_query: RwSignal<Vec<char>> = RwSignal::new(Vec::new());
    let find_current: RwSignal<Option<(usize, usize)>> = RwSignal::new(None);
    let find_seq = StoredValue::new(0u64);
    let selection_at: RwSignal<Option<(f64, f64)>> = RwSignal::new(None);
    let selection: StoredValue<Option<PdfSelection>> = StoredValue::new(None);
    let highlights = highlights.unwrap_or_else(|| Signal::stored(Vec::new()));
    let doc_id: RwSignal<Option<u32>> = RwSignal::new(None);
    let sizes: RwSignal<Vec<(f64, f64)>> = RwSignal::new(Vec::new());
    let width = RwSignal::new(0.0_f64);
    let height = RwSignal::new(0.0_f64);
    let zoom = RwSignal::new(PdfZoom::Width);
    let failed = RwSignal::new(String::new());
    let visible = RwSignal::new((0usize, 2usize));

    let start = StoredValue::new((page, fraction));
    let restored = StoredValue::new(false);
    let kept = StoredValue::new((0usize, 0.0_f64));

    // Close the current document in the shared worker when `url` changes or this view unmounts.
    let active_open: StoredValue<Option<SendWrapper<(Rc<Cell<bool>>, u32)>>> =
        StoredValue::new(None);
    let close_active = move || {
        let _ = active_open.try_update_value(|slot| {
            if let Some(prev) = slot.take() {
                let (alive, id) = prev.take();
                alive.set(false);
                close_doc(id);
            }
        });
    };
    on_cleanup(move || close_active());

    Effect::new(move |_| {
        let current_url = url.get();
        close_active();
        failed.set(String::new());
        doc_id.set(None);
        sizes.set(Vec::new());
        texts.set(HashMap::new());
        text_pending.set_value(HashSet::new());
        find_current.set(None);
        restored.set_value(false);

        let (id, rx) = match start_open(&current_url) {
            Ok(opened) => opened,
            Err(msg) => {
                failed.set(msg.clone());
                if let Some(cb) = on_error {
                    let _ = cb.try_run(msg);
                }
                return;
            }
        };

        let alive = Rc::new(Cell::new(true));
        active_open.set_value(Some(SendWrapper::new((alive.clone(), id))));

        spawn_local(async move {
            let Some(result) = rx.await else {
                return;
            };
            if !alive.get() {
                return;
            }
            match result {
                Ok(opened) => {
                    if opened.pages == 0 {
                        close_doc(id);
                        let msg = "This file is not a readable PDF.".to_owned();
                        let _ = failed.try_set(msg.clone());
                        if let Some(cb) = on_error {
                            let _ = cb.try_run(msg);
                        }
                        return;
                    }
                    let _ = sizes.try_set(opened.sizes);
                    let _ = doc_id.try_set(Some(id));
                    if let Some(cb) = on_ready {
                        let _ = cb.try_run(PdfReady {
                            pages: opened.pages,
                            outline: opened.outline,
                        });
                    }
                }
                Err(msg) => {
                    let _ = failed.try_set(msg.clone());
                    if let Some(cb) = on_error {
                        let _ = cb.try_run(msg);
                    }
                }
            }
        });
    });

    // Observe scroller size.
    Effect::new(move |_| {
        let Some(node) = scroller.get() else { return };
        width.set(node.client_width() as f64);
        height.set(node.client_height() as f64);
        let node_for_cb = node.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let _ = width.try_set(node_for_cb.client_width() as f64);
            let _ = height.try_set(node_for_cb.client_height() as f64);
        });
        let observer =
            web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()).expect("ResizeObserver");
        observer.observe(&node);
        let kept_obs = SendWrapper::new((observer, cb));
        on_cleanup(move || {
            let (observer, _cb) = kept_obs.take();
            observer.disconnect();
        });
    });

    // Compute scale, tops, and total column height.
    let layout = Memo::new(move |_| {
        let s = sizes.get();
        compute_layout(&s, width.get(), height.get(), zoom.get())
    });

    // Set `--pdf-height` via CSSOM (no inline style attribute because of CSP).
    Effect::new(move |_| {
        let l = layout.get();
        if let Some(cb) = on_scale {
            let _ = cb.try_run(l.scale);
        }
        if let Some(node) = pages_node.get() {
            let el: &web_sys::HtmlElement = &node;
            let _ = el
                .style()
                .set_property("--pdf-height", &format!("{}px", l.total.round() as i64));
        }
    });

    let compute_position = move || -> (usize, f64, usize) {
        let Some(node) = scroller.try_get_untracked().flatten() else {
            return (0, 0.0, 0);
        };
        let Some(s) = sizes.try_get_untracked() else {
            return (0, 0.0, 0);
        };
        let Some(l) = layout.try_get_untracked() else {
            return (0, 0.0, 0);
        };
        if l.tops.is_empty() || s.is_empty() {
            return (0, 0.0, 0);
        }
        let scroll_top = node.scroll_top() as f64;
        let client_h = node.client_height() as f64;
        let middle = scroll_top + (client_h / 3.0).min(200.0);
        let mut index = 0usize;
        while index + 1 < l.tops.len() && l.tops[index + 1] <= middle {
            index += 1;
        }
        let h = (s[index].1 * l.scale).max(1.0);
        let frac = ((scroll_top - l.tops[index]) / h).clamp(0.0, 1.0);
        (index, frac, l.tops.len())
    };

    let go_to_fn = move |target: usize, frac: f64| {
        let Some(node) = scroller.try_get_untracked().flatten() else {
            return;
        };
        let Some(s) = sizes.try_get_untracked() else {
            return;
        };
        let Some(l) = layout.try_get_untracked() else {
            return;
        };
        if l.tops.is_empty() || s.is_empty() {
            return;
        }
        let index = target.min(l.tops.len() - 1);
        let offset = if frac != 0.0 { 0.0 } else { PAD / 2.0 };
        let top = l.tops[index] + frac * s[index].1 * l.scale - offset;
        node.set_scroll_top(top.max(0.0) as i32);
    };

    // Keep the same place across zoom and resize; restore the saved place once sizes are known.
    Effect::new(move |_| {
        let current_doc = doc_id.get();
        let w = width.get();
        let l = layout.get();
        if current_doc.is_none() || w <= 0.0 || l.tops.is_empty() {
            return;
        }
        if !restored.get_value() {
            restored.set_value(true);
            let (sp, sf) = start.get_value();
            go_to_fn(sp, sf);
            let (p, f, count) = compute_position();
            kept.set_value((p, f));
            if let Some(cb) = on_position {
                let _ = cb.try_run((p, f, count));
            }
        } else {
            let (kp, kf) = kept.get_value();
            go_to_fn(kp, kf);
        }
    });

    // Update visible window and report scroll position (rAF-throttled).
    let update_visible = move || {
        let Some(node) = scroller.try_get_untracked().flatten() else {
            return;
        };
        let Some(l) = layout.try_get_untracked() else {
            return;
        };
        if l.tops.is_empty() {
            return;
        }
        let st = node.scroll_top() as f64;
        let ch = node.client_height() as f64;
        let top = st - ch;
        let bottom = st + ch * 2.0;
        let mut a = 0usize;
        while a + 1 < l.tops.len() && l.tops[a + 1] < top {
            a += 1;
        }
        let mut b = a;
        while b + 1 < l.tops.len() && l.tops[b + 1] < bottom {
            b += 1;
        }
        if visible.try_get_untracked() != Some((a, b)) {
            let _ = visible.try_set((a, b));
        }
    };

    Effect::new(move |_| {
        let _ = layout.get();
        let _ = height.get();
        update_visible();
    });

    // Attach scroll listener on scroller.
    Effect::new(move |_| {
        let Some(node) = scroller.get() else { return };
        let frame_id: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
        let raf_cb: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
        let frame_id_listener = frame_id.clone();
        let raf_cb_listener = raf_cb.clone();
        let listener = Listener::new(node.as_ref(), "scroll", false, move |_| {
            update_visible();
            if let Some(id) = frame_id_listener.take() {
                let _ = window().cancel_animation_frame(id);
            }
            let fid = frame_id_listener.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                fid.set(None);
                if !restored.try_get_value().unwrap_or(false) {
                    return;
                }
                let (p, f, count) = compute_position();
                let _ = kept.try_set_value((p, f));
                if let Some(cb) = on_position {
                    let _ = cb.try_run((p, f, count));
                }
            });
            if let Ok(id) = window().request_animation_frame(cb.as_ref().unchecked_ref()) {
                frame_id_listener.set(Some(id));
                *raf_cb_listener.borrow_mut() = Some(cb);
            }
        });
        let held = SendWrapper::new((listener, frame_id, raf_cb));
        on_cleanup(move || {
            let (_listener, frame_id, _raf_cb) = held.take();
            if let Some(id) = frame_id.take() {
                let _ = window().cancel_animation_frame(id);
            }
        });
    });

    // Page text: asked for once per page and document, kept for the text layer and Find.
    let ensure_text = Callback::new(move |index: usize| {
        let Some(current_doc) = doc_id.try_get_untracked().flatten() else {
            return;
        };
        if texts.with_untracked(|t| t.contains_key(&index)) {
            return;
        }
        if !text_pending
            .try_update_value(|p| p.insert(index))
            .unwrap_or(false)
        {
            return;
        }
        spawn_local(async move {
            let runs = page_text(current_doc, index).await;
            if doc_id.try_get_untracked().flatten() == Some(current_doc) {
                let _ = texts.try_update(|t| {
                    t.insert(index, Arc::new(runs));
                });
            }
        });
    });

    // Find: matches over the pages read so far, in page order.
    let matches = Memo::new(move |_| {
        let query = find_query.get();
        if query.is_empty() {
            return Vec::new();
        }
        let page_sizes = sizes.get();
        texts.with(|t| {
            let mut pages: Vec<_> = t.iter().collect();
            pages.sort_by_key(|(page, _)| **page);
            pages
                .into_iter()
                .flat_map(|(page, runs)| {
                    let size = page_sizes.get(*page).copied().unwrap_or((612.0, 792.0));
                    find_on_page(*page, runs, &query, size)
                })
                .collect::<Vec<PdfMatch>>()
        })
    });
    let current_index = Memo::new(move |_| {
        let current = find_current.get()?;
        matches.with(|m| m.iter().position(|x| (x.page, x.ordinal) == current))
    });
    let scroll_to_match = move |found: &PdfMatch| {
        let Some(node) = scroller.try_get_untracked().flatten() else {
            return;
        };
        let Some(l) = layout.try_get_untracked() else {
            return;
        };
        let Some(top) = l.tops.get(found.page) else {
            return;
        };
        let ph = sizes.with_untracked(|s| s.get(found.page).map_or(792.0, |s| s.1));
        let y = found.rects.first().map_or(0.0, |r| r.y);
        let target = top + y * ph * l.scale - f64::from(node.client_height()) / 3.0;
        node.set_scroll_top(target.max(0.0) as i32);
    };
    // The first match at or after the current page becomes current, once every page up to it
    // has been read (so a later page read early never jumps ahead of a nearer match).
    Effect::new(move |_| {
        let list = matches.get();
        if find_current
            .get_untracked()
            .is_some_and(|c| list.iter().any(|m| (m.page, m.ordinal) == c))
        {
            return;
        }
        if list.is_empty() {
            let _ = find_current.try_set(None);
            return;
        }
        let here = kept.try_get_value().map_or(0, |(p, _)| p);
        let read = |page: usize| texts.with_untracked(|t| t.contains_key(&page));
        let chosen = match list.iter().find(|m| m.page >= here) {
            Some(m) if (here..m.page).all(read) => Some(m.clone()),
            Some(_) => None,
            None if (0..sizes.with_untracked(Vec::len)).all(read) => list.first().cloned(),
            None => None,
        };
        let Some(found) = chosen else { return };
        let _ = find_current.try_set(Some((found.page, found.ordinal)));
        scroll_to_match(&found);
    });
    if let Some(cb) = on_find {
        Effect::new(move |_| {
            let count = matches.with(Vec::len);
            let current = current_index.get().map(|i| i + 1);
            let searching = !find_query.with(Vec::is_empty);
            let total = sizes.with(Vec::len);
            let complete = !searching || texts.with(|t| t.len() >= total);
            let _ = cb.try_run(PdfFindState {
                matches: count,
                current,
                complete,
            });
        });
    }
    let run_find = move |query: String| {
        let folded = fold_query(&query);
        let seq = find_seq.get_value() + 1;
        find_seq.set_value(seq);
        let _ = find_current.try_set(None);
        let _ = find_query.try_set(folded.clone());
        let Some(current_doc) = doc_id.try_get_untracked().flatten() else {
            return;
        };
        if folded.is_empty() {
            return;
        }
        // Read the pages not read yet, one at a time from this page on, so drawing stays ahead
        // of searching.
        let here = kept.try_get_value().map_or(0, |(p, _)| p);
        spawn_local(async move {
            let total = sizes.try_with_untracked(Vec::len).unwrap_or(0);
            for index in (here.min(total)..total).chain(0..here.min(total)) {
                if find_seq.try_get_value() != Some(seq)
                    || doc_id.try_get_untracked().flatten() != Some(current_doc)
                {
                    return;
                }
                if texts
                    .try_with_untracked(|t| t.contains_key(&index))
                    .unwrap_or(true)
                {
                    continue;
                }
                let runs = page_text(current_doc, index).await;
                if doc_id.try_get_untracked().flatten() != Some(current_doc) {
                    return;
                }
                let _ = texts.try_update(|t| {
                    t.entry(index).or_insert_with(|| Arc::new(runs));
                });
            }
        });
    };
    let step_find = move |direction: i32| {
        let Some(list) = matches.try_get_untracked() else {
            return;
        };
        if list.is_empty() {
            return;
        }
        let len = list.len() as i64;
        let next = match current_index.try_get_untracked().flatten() {
            Some(i) => (i as i64 + i64::from(direction)).rem_euclid(len) as usize,
            None if direction < 0 => list.len() - 1,
            None => 0,
        };
        let found = &list[next];
        let _ = find_current.try_set(Some((found.page, found.ordinal)));
        scroll_to_match(found);
    };

    // Selecting text offers Highlight (Books); the action sits under the selection's end.
    if on_highlight.is_some() {
        listen(&document(), "selectionchange", false, move |_| {
            let Some(node) = scroller.try_get_untracked().flatten() else {
                return;
            };
            let found = current_selection(&node);
            let place = found.as_ref().and_then(|_| {
                let pages = pages_node.try_get_untracked().flatten()?;
                let frame = pages.get_bounding_client_rect();
                let sel = window().get_selection().ok().flatten()?;
                let range = sel.get_range_at(sel.range_count().checked_sub(1)?).ok()?;
                let end = range.get_bounding_client_rect();
                Some((end.right() - frame.left(), end.bottom() - frame.top() + 6.0))
            });
            let _ = selection.try_set_value(found);
            if selection_at.try_get_untracked() != Some(place) {
                let _ = selection_at.try_set(place);
            }
        });
    }
    let take_highlight = move || {
        let Some(found) = selection.try_update_value(|s| s.take()).flatten() else {
            return;
        };
        if let Some(sel) = window().get_selection().ok().flatten() {
            let _ = sel.remove_all_ranges();
        }
        let _ = selection_at.try_set(None);
        if let Some(cb) = on_highlight {
            let _ = cb.try_run(found);
        }
    };

    // Expose PdfHandle if caller wants imperative control.
    if let Some(slot) = handle {
        let change_zoom = move |next: PdfZoom| {
            let _ = zoom.try_set(next);
            if let Some(cb) = on_zoom {
                let _ = cb.try_run(next);
            }
        };
        slot.set(Some(PdfHandle {
            go_to: Callback::new(move |(p, f)| go_to_fn(p, f)),
            step: Callback::new(move |direction: i32| {
                if let Some(node) = scroller.try_get_untracked().flatten() {
                    let reduced = window()
                        .match_media("(prefers-reduced-motion: reduce)")
                        .ok()
                        .flatten()
                        .is_some_and(|m| m.matches());
                    let opts = web_sys::ScrollToOptions::new();
                    opts.set_top((direction as f64) * (node.client_height() as f64) * 0.9);
                    opts.set_behavior(if reduced {
                        web_sys::ScrollBehavior::Auto
                    } else {
                        web_sys::ScrollBehavior::Smooth
                    });
                    node.scroll_by_with_scroll_to_options(&opts);
                }
            }),
            zoom: Callback::new(move |change: PdfZoomChange| match change {
                PdfZoomChange::Width => change_zoom(PdfZoom::Width),
                PdfZoomChange::Page => change_zoom(PdfZoom::Page),
                PdfZoomChange::In => {
                    if let Some(s) = layout.try_with_untracked(|l| l.scale) {
                        change_zoom(PdfZoom::Scale((s * 1.25).clamp(0.25, 5.0)));
                    }
                }
                PdfZoomChange::Out => {
                    if let Some(s) = layout.try_with_untracked(|l| l.scale) {
                        change_zoom(PdfZoom::Scale((s * 0.8).clamp(0.25, 5.0)));
                    }
                }
            }),
            find: Callback::new(run_find),
            find_step: Callback::new(step_find),
        }));
        on_cleanup(move || {
            let _ = slot.try_set(None);
        });
    }

    let visible_indices = Memo::new(move |_| {
        let len = sizes.with(Vec::len);
        if len == 0 {
            return Vec::new();
        }
        let (a, b) = visible.get();
        let start_i = a.min(len - 1);
        let end_i = b.min(len - 1);
        (start_i..=end_i).collect::<Vec<_>>()
    });

    let root_class = move || {
        let t = tint.map(|s| s.get()).unwrap_or_else(|| "paper".to_owned());
        format!("pdf-view pdf-tint-{t} {class_name}")
    };

    view! {
        <div
            node_ref=scroller
            class=root_class
            tabindex="0"
            role="document"
            aria-label=move || label.get()
            aria-busy=move || (doc_id.with(Option::is_none) && failed.with(String::is_empty)).to_string()
        >
            {move || {
                let err = failed.get();
                if !err.is_empty() {
                    view! { <div class="pdf-view-message" role="alert">{err}</div> }.into_any()
                } else if doc_id.with(Option::is_none) {
                    view! { <div class="pdf-view-message" role="status">"Opening PDF\u{2026}"</div> }.into_any()
                } else {
                    view! {
                        <div node_ref=pages_node class="pdf-view-pages">
                            <For
                                each=move || visible_indices.get()
                                key=|index| *index
                                children=move |index| {
                                    view! {
                                        <PdfPage
                                            doc_id=doc_id
                                            sizes=sizes
                                            layout=layout
                                            index=index
                                            texts=texts
                                            ensure_text=ensure_text
                                            highlights=highlights
                                            matches=matches
                                            current=find_current
                                        />
                                    }
                                }
                            />
                            {move || {
                                let (x, y) = selection_at.get()?;
                                let place = NodeRef::<html::Button>::new();
                                Effect::new(move |_| {
                                    if let Some(button) = place.get() {
                                        let style = web_sys::HtmlElement::style(&button);
                                        let _ = style.set_property("--pdf-action-x", &format!("{x}px"));
                                        let _ = style.set_property("--pdf-action-y", &format!("{y}px"));
                                    }
                                });
                                Some(view! {
                                    <button
                                        node_ref=place
                                        type="button"
                                        class="pdf-view-action"
                                        on:pointerdown=|event| event.prevent_default()
                                        on:click=move |_| take_highlight()
                                    >
                                        <Icon name="quote"/>
                                        "Highlight"
                                    </button>
                                })
                            }}
                        </div>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}

struct RenderGuard {
    alive: Rc<Cell<bool>>,
    req_id: Rc<Cell<Option<u32>>>,
}

impl RenderGuard {
    fn cancel(&self) {
        self.alive.set(false);
        if let Some(id) = self.req_id.take() {
            cancel_render(id);
        }
    }
}

/// One page box; its canvas is drawn at the screen's pixel density.
///
/// While re-rendering after a zoom change, the previous bitmap stays on the canvas and is scaled
/// smoothly by CSS (`width: 100%; height: 100%`) until the crisp replacement arrives.
#[component]
fn PdfPage(
    doc_id: RwSignal<Option<u32>>,
    sizes: RwSignal<Vec<(f64, f64)>>,
    layout: Memo<Layout>,
    index: usize,
    texts: RwSignal<HashMap<usize, Arc<Vec<PdfTextRun>>>>,
    ensure_text: Callback<usize>,
    highlights: Signal<Vec<PdfHighlight>>,
    matches: Memo<Vec<PdfMatch>>,
    current: RwSignal<Option<(usize, usize)>>,
) -> impl IntoView {
    let box_ref = NodeRef::<html::Div>::new();
    let canvas_ref = NodeRef::<html::Canvas>::new();
    let text_ref = NodeRef::<html::Div>::new();
    let marks_ref = NodeRef::<html::Div>::new();
    let page_runs = Memo::new(move |_| texts.with(|t| t.get(&index).cloned()));
    let has_text =
        Memo::new(move |_| page_runs.with(|r| r.as_ref().is_some_and(|r| !r.is_empty())));

    // The text layer: asked for with the page, rebuilt only when the text arrives.
    Effect::new(move |_| {
        if doc_id.get().is_some() && page_runs.with_untracked(Option::is_none) {
            ensure_text.run(index);
        }
    });
    Effect::new(move |_| {
        let Some(node) = text_ref.get() else { return };
        page_runs.with(|runs| build_text_layer(&node, runs.as_deref().map_or(&[], Vec::as_slice)));
    });
    // Highlights, then search matches (the current one stands out).
    Effect::new(move |_| {
        let Some(node) = marks_ref.get() else { return };
        let mut marks = Vec::new();
        highlights.with(|list| {
            for h in list {
                marks.extend(
                    h.rects
                        .iter()
                        .filter(|r| r.page == index)
                        .map(|r| (*r, "pdf-view-highlight")),
                );
            }
        });
        let now = current.get();
        matches.with(|list| {
            for m in list.iter().filter(|m| m.page == index) {
                let class = if now == Some((m.page, m.ordinal)) {
                    "pdf-view-match is-current"
                } else {
                    "pdf-view-match"
                };
                marks.extend(m.rects.iter().map(|r| (*r, class)));
            }
        });
        paint_marks(&node, &marks);
    });
    let drawn = RwSignal::new(false);
    let page_error = RwSignal::new(String::new());
    let drawn_doc: StoredValue<Option<u32>> = StoredValue::new(None);

    // Update page box geometry via CSSOM.
    Effect::new(move |_| {
        let Some(node) = box_ref.get() else { return };
        let l = layout.get();
        let (pw, ph) = sizes.with(|s| s.get(index).copied().unwrap_or((0.0, 0.0)));
        let top = l.tops.get(index).copied().unwrap_or(0.0);
        let style = web_sys::HtmlElement::style(&node);
        let _ = style.set_property("--pdf-page-top", &format!("{top}px"));
        let _ = style.set_property("--pdf-page-width", &format!("{}px", pw * l.scale));
        let _ = style.set_property("--pdf-page-height", &format!("{}px", ph * l.scale));
        let _ = style.set_property("--pdf-scale", &format!("{}", l.scale));
    });

    // Request render from the shared worker after 60ms settle; cancel on zoom/doc change or unmount.
    let guard: StoredValue<Option<SendWrapper<RenderGuard>>> = StoredValue::new(None);
    let cancel_current = move || {
        let _ = guard.try_update_value(|slot| {
            if let Some(prev) = slot.take() {
                prev.take().cancel();
            }
        });
    };
    on_cleanup(move || cancel_current());

    Effect::new(move |_| {
        let Some(current_doc) = doc_id.get() else {
            return;
        };
        let scale = layout.with(|l| l.scale);
        let (pw, ph) = sizes.with_untracked(|s| s.get(index).copied().unwrap_or((612.0, 792.0)));
        cancel_current();
        if drawn_doc.get_value() != Some(current_doc) {
            drawn_doc.set_value(Some(current_doc));
            drawn.set(false);
            page_error.set(String::new());
        }

        let alive = Rc::new(Cell::new(true));
        let req_slot: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
        guard.set_value(Some(SendWrapper::new(RenderGuard {
            alive: alive.clone(),
            req_id: req_slot.clone(),
        })));

        spawn_local(async move {
            crate::sleep(60).await;
            if !alive.get() {
                return;
            }
            let dpr = window().device_pixel_ratio();
            let ratio = if dpr > 0.0 { dpr.min(2.0) } else { 1.0 };
            let max_scale = (MAX_RENDER_PIXELS / (pw * ph).max(1.0)).sqrt();
            let render_scale = (scale * ratio).min(max_scale).max(0.1);

            let (req_id, rx) = match start_render(current_doc, index, render_scale) {
                Ok(started) => started,
                Err(msg) => {
                    if !drawn.try_get_untracked().unwrap_or(false) {
                        let _ = page_error.try_set(msg);
                    }
                    return;
                }
            };
            req_slot.set(Some(req_id));

            let Some(result) = rx.await else {
                req_slot.set(None);
                return;
            };
            req_slot.set(None);
            if !alive.get() {
                return;
            }
            match result {
                Ok(rendered) => {
                    let Some(target) = canvas_ref.try_get_untracked().flatten() else {
                        return;
                    };
                    let Ok(Some(ctx_obj)) = target.get_context("2d") else {
                        return;
                    };
                    let Ok(ctx) = ctx_obj.dyn_into::<web_sys::CanvasRenderingContext2d>() else {
                        return;
                    };
                    let Ok(image) =
                        image_data_from_buffer(&rendered.buffer, rendered.width, rendered.height)
                    else {
                        return;
                    };
                    target.set_width(rendered.width);
                    target.set_height(rendered.height);
                    if ctx.put_image_data(&image, 0.0, 0.0).is_ok() {
                        let _ = page_error.try_set(String::new());
                        let _ = drawn.try_set(true);
                    }
                }
                Err(msg) => {
                    if !drawn.try_get_untracked().unwrap_or(false) {
                        let _ = page_error.try_set(msg);
                    }
                }
            }
        });
    });

    view! {
        <div
            node_ref=box_ref
            class=move || if drawn.get() { "pdf-view-page is-drawn" } else { "pdf-view-page" }
            aria-label=format!("Page {}", index + 1)
            role=move || if has_text.get() { "group" } else { "img" }
            data-page=index.to_string()
        >
            <canvas node_ref=canvas_ref aria-hidden="true"/>
            <div node_ref=marks_ref class="pdf-view-marks" aria-hidden="true"></div>
            <div node_ref=text_ref class="pdf-view-text"></div>
            {move || {
                let err = page_error.get();
                (!err.is_empty() && !drawn.get()).then(|| {
                    view! { <div class="pdf-view-message" role="alert">{err}</div> }
                })
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_computes_scales_and_page_tops() {
        let sizes = vec![(300.0, 400.0), (600.0, 800.0)];

        let fit_w = compute_layout(&sizes, 632.0, 800.0, PdfZoom::Width);
        assert!((fit_w.scale - 1.0).abs() < 1e-6);
        assert_eq!(fit_w.tops, vec![PAD, PAD + 400.0 + GAP]);
        assert_eq!(fit_w.total, PAD + 400.0 + GAP + 800.0 + GAP);

        let fit_p = compute_layout(&sizes, 1000.0, 432.0, PdfZoom::Page);
        assert!((fit_p.scale - 0.5).abs() < 1e-6);

        let fixed = compute_layout(&sizes, 100.0, 100.0, PdfZoom::Scale(1.5));
        assert!((fixed.scale - 1.5).abs() < 1e-6);
        assert_eq!(fixed.tops, vec![PAD, PAD + 600.0 + GAP]);
    }
}
