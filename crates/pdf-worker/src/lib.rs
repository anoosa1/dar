//! Dedicated Web Worker running `dar-pdf` in WebAssembly for Dar's browser PDF viewer.
//!
//! One worker instance is shared across all PDF viewers in a browser tab (Books reader and
//! Files preview). Documents are stored by caller-chosen numeric IDs so multiple viewers can
//! keep PDFs open simultaneously without interfering with one another.
//!
//! # Message protocol
//!
//! All messages are plain JavaScript objects sent via `postMessage`.
//!
//! ## Main thread → Worker
//!
//! - **Open a document:**
//!   `{ type: "open", id: number, url: string }`
//!   Fetches `url` with `credentials: "same-origin"`, parses the PDF with [`dar_pdf::Document::open`],
//!   stores it under `id`, and replies with `"opened"` or `"error"`. If `"close"` for `id` arrives
//!   before the fetch completes, the result is discarded.
//!
//! - **Queue a page render:**
//!   `{ type: "render", doc: number, page: number, scale: number, request: number }`
//!   Queues page `page` (zero-based) of document `doc` at `scale` device pixels per PDF point.
//!   Queued renders run LIFO (most recent/visible requests first), yielding via `setTimeout(..., 0)`
//!   between pages so incoming `"cancel"`, `"close"`, `"open"`, and newer `"render"` messages are
//!   processed before the next page begins. Replies with `"page"` or `"render-error"`.
//!
//! - **Cancel a queued render:**
//!   `{ type: "cancel", request: number }`
//!   Removes `request` from the pending render queue if it has not started yet. No reply is sent.
//!
//! - **Close a document:**
//!   `{ type: "close", doc: number }`
//!   Drops the open [`dar_pdf::Document`] (or cancels an in-flight `"open"`), and discards any
//!   queued renders belonging to `doc`. No reply is sent.
//!
//! ## Worker → Main thread
//!
//! - **Document opened:**
//!   `{ type: "opened", id: number, pages: number, sizes: [number, number][], outline: { title: string, page: number | null, depth: number }[] }`
//!   `sizes` are `[width, height]` in PDF points; `outline` entries are flattened depth-first with
//!   zero-based `page` or `null` when unresolved.
//!
//! - **Open failed:**
//!   `{ type: "error", id: number, message: string }`
//!
//! - **Page rendered:**
//!   `{ type: "page", request: number, width: number, height: number, buffer: ArrayBuffer }`
//!   `buffer` holds straight RGBA8 pixel bytes (`width * height * 4` bytes on white) and is
//!   transferred (zero-copy) in the `postMessage` transfer list.
//!
//! - **Render failed:**
//!   `{ type: "render-error", request: number, message: string }`
//!
//! - **Startup failure (emitted by `worker.js` bootstrap):**
//!   `{ type: "fatal", message: string }`

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use dar_pdf::{Document, OutlineEntry, PageSize};
use js_sys::{Array, ArrayBuffer, Object, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{DedicatedWorkerGlobalScope, RequestCredentials, RequestInit, Response};

/// A queued page render request.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RenderJob {
    request: u32,
    doc: u32,
    page: usize,
    scale: f32,
}

/// LIFO render queue supporting cancellation by request ID and pruning by document ID.
#[derive(Default)]
struct RenderQueue {
    jobs: Vec<RenderJob>,
}

impl RenderQueue {
    fn push(&mut self, job: RenderJob) {
        // Avoid duplicates if the same request ID is somehow re-sent.
        self.jobs.retain(|existing| existing.request != job.request);
        self.jobs.push(job);
    }

    fn pop_next(&mut self) -> Option<RenderJob> {
        self.jobs.pop()
    }

    fn cancel(&mut self, request: u32) {
        self.jobs.retain(|job| job.request != request);
    }

    fn remove_doc(&mut self, doc: u32) {
        self.jobs.retain(|job| job.doc != doc);
    }

    fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

struct WorkerState {
    docs: HashMap<u32, Document>,
    pending_opens: HashSet<u32>,
    queue: RenderQueue,
    drain_scheduled: bool,
    drain_callback: Option<Closure<dyn FnMut()>>,
}

impl WorkerState {
    fn new() -> Self {
        Self {
            docs: HashMap::new(),
            pending_opens: HashSet::new(),
            queue: RenderQueue::default(),
            drain_scheduled: false,
            drain_callback: None,
        }
    }
}

thread_local! {
    static STATE: RefCell<WorkerState> = RefCell::new(WorkerState::new());
}

fn scope() -> DedicatedWorkerGlobalScope {
    js_sys::global().unchecked_into()
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

fn post(msg: &Object) {
    let _ = scope().post_message(msg);
}

fn post_open_error(id: u32, message: &str) {
    let msg = Object::new();
    set_prop(&msg, "type", &JsValue::from_str("error"));
    set_prop(&msg, "id", &JsValue::from_f64(id as f64));
    set_prop(&msg, "message", &JsValue::from_str(message));
    post(&msg);
}

fn post_opened(id: u32, pages: usize, sizes: &[PageSize], outline: &[OutlineEntry]) {
    let sizes_arr = Array::new_with_length(sizes.len() as u32);
    for (i, size) in sizes.iter().enumerate() {
        let pair = Array::new_with_length(2);
        pair.set(0, JsValue::from_f64(size.width as f64));
        pair.set(1, JsValue::from_f64(size.height as f64));
        sizes_arr.set(i as u32, pair.into());
    }

    let outline_arr = Array::new_with_length(outline.len() as u32);
    for (i, entry) in outline.iter().enumerate() {
        let item = Object::new();
        set_prop(&item, "title", &JsValue::from_str(&entry.title));
        match entry.page {
            Some(page) => set_prop(&item, "page", &JsValue::from_f64(page as f64)),
            None => set_prop(&item, "page", &JsValue::NULL),
        }
        set_prop(&item, "depth", &JsValue::from_f64(entry.depth as f64));
        outline_arr.set(i as u32, item.into());
    }

    let msg = Object::new();
    set_prop(&msg, "type", &JsValue::from_str("opened"));
    set_prop(&msg, "id", &JsValue::from_f64(id as f64));
    set_prop(&msg, "pages", &JsValue::from_f64(pages as f64));
    set_prop(&msg, "sizes", &sizes_arr);
    set_prop(&msg, "outline", &outline_arr);
    post(&msg);
}

fn post_render_error(request: u32, message: &str) {
    let msg = Object::new();
    set_prop(&msg, "type", &JsValue::from_str("render-error"));
    set_prop(&msg, "request", &JsValue::from_f64(request as f64));
    set_prop(&msg, "message", &JsValue::from_str(message));
    post(&msg);
}

fn post_page(request: u32, width: u32, height: u32, rgba: &[u8]) {
    // Copy out of WASM linear memory into a standalone ArrayBuffer so it can be transferred.
    let bytes = Uint8Array::from(rgba);
    let buffer: ArrayBuffer = bytes.buffer();
    let msg = Object::new();
    set_prop(&msg, "type", &JsValue::from_str("page"));
    set_prop(&msg, "request", &JsValue::from_f64(request as f64));
    set_prop(&msg, "width", &JsValue::from_f64(width as f64));
    set_prop(&msg, "height", &JsValue::from_f64(height as f64));
    set_prop(&msg, "buffer", &buffer);

    let transfer = Array::of1(&buffer);
    let _ = scope().post_message_with_transfer(&msg, &transfer);
}

/// Entry point called by `worker.js` for every message received from the main thread.
#[wasm_bindgen]
pub fn handle_message(data: JsValue) {
    let Some(kind) = get_str(&data, "type") else {
        return;
    };
    match kind.as_str() {
        "open" => {
            let Some(id) = get_u32(&data, "id") else { return };
            let Some(url) = get_str(&data, "url") else {
                post_open_error(id, "The PDF could not be loaded.");
                return;
            };
            start_open(id, url);
        }
        "render" => {
            let Some(request) = get_u32(&data, "request") else { return };
            let Some(doc) = get_u32(&data, "doc") else {
                post_render_error(request, "Invalid render request.");
                return;
            };
            let Some(page) = get_u32(&data, "page") else {
                post_render_error(request, "Invalid page index.");
                return;
            };
            let Some(scale) = get_f64(&data, "scale") else {
                post_render_error(request, "Invalid render scale.");
                return;
            };
            if !scale.is_finite() || scale <= 0.0 {
                post_render_error(request, "Invalid render scale.");
                return;
            }
            STATE.with(|cell| {
                cell.borrow_mut().queue.push(RenderJob {
                    request,
                    doc,
                    page: page as usize,
                    scale: scale as f32,
                });
            });
            schedule_drain();
        }
        "cancel" => {
            if let Some(request) = get_u32(&data, "request") {
                STATE.with(|cell| cell.borrow_mut().queue.cancel(request));
            }
        }
        "close" => {
            if let Some(doc) = get_u32(&data, "doc") {
                STATE.with(|cell| {
                    let mut state = cell.borrow_mut();
                    state.docs.remove(&doc);
                    state.pending_opens.remove(&doc);
                    state.queue.remove_doc(doc);
                });
            }
        }
        _ => {}
    }
}

fn start_open(id: u32, url: String) {
    STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        state.docs.remove(&id);
        state.queue.remove_doc(id);
        state.pending_opens.insert(id);
    });

    spawn_local(async move {
        let fetched = fetch_bytes(&url).await;
        let still_pending = STATE.with(|cell| cell.borrow().pending_opens.contains(&id));
        if !still_pending {
            return;
        }

        let bytes = match fetched {
            Ok(bytes) => bytes,
            Err(message) => {
                STATE.with(|cell| {
                    cell.borrow_mut().pending_opens.remove(&id);
                });
                post_open_error(id, &message);
                return;
            }
        };

        match Document::open(bytes) {
            Ok(doc) => {
                let pages = doc.page_count();
                let sizes = doc.page_sizes();
                let outline = doc.outline();
                let keep = STATE.with(|cell| {
                    let mut state = cell.borrow_mut();
                    if state.pending_opens.remove(&id) {
                        state.docs.insert(id, doc);
                        true
                    } else {
                        false
                    }
                });
                if keep {
                    post_opened(id, pages, &sizes, &outline);
                }
            }
            Err(err) => {
                let send = STATE.with(|cell| cell.borrow_mut().pending_opens.remove(&id));
                if send {
                    post_open_error(id, &err.to_string());
                }
            }
        }
    });
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let init = RequestInit::new();
    init.set_credentials(RequestCredentials::SameOrigin);
    let response_value = JsFuture::from(scope().fetch_with_str_and_init(url, &init))
        .await
        .map_err(|_| "The PDF could not be loaded.".to_owned())?;
    let response: Response = response_value
        .dyn_into()
        .map_err(|_| "The PDF could not be loaded.".to_owned())?;
    if !response.ok() {
        return Err("The PDF could not be loaded.".to_owned());
    }
    let buffer_promise = response
        .array_buffer()
        .map_err(|_| "The PDF could not be loaded.".to_owned())?;
    let buffer = JsFuture::from(buffer_promise)
        .await
        .map_err(|_| "The PDF could not be loaded.".to_owned())?;
    Ok(Uint8Array::new(&buffer).to_vec())
}

fn schedule_drain() {
    let should_schedule = STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        if state.drain_scheduled || state.queue.is_empty() {
            return false;
        }
        state.drain_scheduled = true;
        if state.drain_callback.is_none() {
            state.drain_callback = Some(Closure::new(drain_one));
        }
        true
    });
    if !should_schedule {
        return;
    }
    STATE.with(|cell| {
        let state = cell.borrow();
        if let Some(cb) = state.drain_callback.as_ref() {
            let _ = scope().set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                0,
            );
        }
    });
}

fn drain_one() {
    let next_job = STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        state.drain_scheduled = false;
        loop {
            let job = state.queue.pop_next()?;
            if state.docs.contains_key(&job.doc) {
                return Some(job);
            }
        }
    });

    let Some(job) = next_job else {
        return;
    };

    let rendered = STATE.with(|cell| {
        let state = cell.borrow();
        let doc = state.docs.get(&job.doc)?;
        if job.page >= doc.page_count() {
            return Some(Err("Page out of range.".to_owned()));
        }
        Some(doc.render(job.page, job.scale).map_err(|e| e.to_string()))
    });

    match rendered {
        Some(Ok(bitmap)) => {
            let expected = (bitmap.width as usize)
                .saturating_mul(bitmap.height as usize)
                .saturating_mul(4);
            if bitmap.width == 0 || bitmap.height == 0 || bitmap.rgba.len() != expected {
                post_render_error(job.request, "This page could not be drawn.");
            } else {
                post_page(job.request, bitmap.width, bitmap.height, &bitmap.rgba);
            }
        }
        Some(Err(message)) => {
            post_render_error(job.request, &message);
        }
        None => {}
    }

    // Yield back to the event loop before taking the next queued render so cancellations
    // or newer visible-page requests jump ahead of stale work.
    schedule_drain();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_queue_is_lifo_and_honors_cancel_and_close() {
        let mut q = RenderQueue::default();
        q.push(RenderJob { request: 1, doc: 10, page: 0, scale: 1.0 });
        q.push(RenderJob { request: 2, doc: 10, page: 1, scale: 1.0 });
        q.push(RenderJob { request: 3, doc: 20, page: 0, scale: 1.5 });
        q.push(RenderJob { request: 4, doc: 10, page: 2, scale: 1.0 });

        q.cancel(2);
        assert_eq!(q.pop_next().map(|j| j.request), Some(4));

        q.remove_doc(10);
        assert_eq!(q.pop_next().map(|j| j.request), Some(3));
        assert!(q.is_empty());
        assert_eq!(q.pop_next(), None);
    }
}
