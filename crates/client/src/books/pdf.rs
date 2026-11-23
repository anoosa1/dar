//! PDF viewer backed by pdf.js (`next/client/books/pdf.tsx`).
//!
//! Pages are laid out at their real proportions in one scrolling column and only pages
//! near the viewport are drawn on canvases. Reused by both Books and Files preview.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use js_sys::{Array, Function, Object, Promise, Reflect};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::ui::{call_method, window, Listener};

// Keep in step with the copy the client dist gets (node_modules/pdfjs-dist/package.json; xtask/src/client.rs).
const ASSETS: &str = "/pdfjs/6.5.438/";
const GAP: f64 = 12.0;
const PAD: f64 = 16.0;

#[wasm_bindgen(inline_js = "export function dynamic_import(u) { return import(u); }")]
extern "C" {
    fn dynamic_import(u: &str) -> js_sys::Promise;
}

thread_local! {
    static PDFJS_PROMISE: RefCell<Option<Promise>> = const { RefCell::new(None) };
}

async fn load_pdfjs() -> Result<JsValue, JsValue> {
    let promise = PDFJS_PROMISE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(p) = slot.as_ref() {
            return p.clone();
        }
        let url = format!("{ASSETS}pdf.min.mjs");
        let p = dynamic_import(&url);
        *slot = Some(p.clone());
        p
    });
    let module = match JsFuture::from(promise).await {
        Ok(m) => m,
        Err(err) => {
            PDFJS_PROMISE.with(|cell| *cell.borrow_mut() = None);
            return Err(err);
        }
    };
    if let Ok(opts) = Reflect::get(&module, &JsValue::from_str("GlobalWorkerOptions"))
        && !opts.is_undefined()
        && !opts.is_null()
    {
        let worker = format!("{ASSETS}pdf.worker.min.mjs");
        let _ = Reflect::set(&opts, &JsValue::from_str("workerSrc"), &JsValue::from_str(&worker));
    }
    Ok(module)
}

fn call1(target: &JsValue, name: &str, arg: &JsValue) -> Result<JsValue, JsValue> {
    let func: Function = Reflect::get(target, &JsValue::from_str(name))?.dyn_into()?;
    func.call1(target, arg)
}

fn call0(target: &JsValue, name: &str) -> Result<JsValue, JsValue> {
    let func: Function = Reflect::get(target, &JsValue::from_str(name))?.dyn_into()?;
    func.call0(target)
}

async fn get_page_size(doc: &JsValue, page_num: usize) -> Result<(f64, f64), JsValue> {
    let page_promise: Promise = call1(doc, "getPage", &JsValue::from_f64(page_num as f64))?.dyn_into()?;
    let page = JsFuture::from(page_promise).await?;
    let opts = Object::new();
    let _ = Reflect::set(&opts, &JsValue::from_str("scale"), &JsValue::from_f64(1.0));
    let viewport = call1(&page, "getViewport", &opts)?;
    let w = Reflect::get(&viewport, &JsValue::from_str("width"))?.as_f64().unwrap_or(612.0);
    let h = Reflect::get(&viewport, &JsValue::from_str("height"))?.as_f64().unwrap_or(792.0);
    Ok((w, h))
}

fn walk_outline<'a>(
    doc: &'a JsValue,
    items: JsValue,
    depth: usize,
    out: &'a mut Vec<PdfOutlineEntry>,
) -> Pin<Box<dyn Future<Output = ()> + 'a>> {
    Box::pin(async move {
        if items.is_null() || items.is_undefined() || out.len() >= 2000 {
            return;
        }
        let arr = Array::from(&items);
        let len = arr.length();
        for i in 0..len {
            if out.len() >= 2000 {
                return;
            }
            let item = arr.get(i);
            let mut target: i32 = -1;
            if let Ok(dest) = Reflect::get(&item, &JsValue::from_str("dest")) {
                let resolved = if let Some(name) = dest.as_string() {
                    if let Ok(p) = call1(doc, "getDestination", &JsValue::from_str(&name))
                        && let Ok(promise) = p.dyn_into::<Promise>()
                    {
                        JsFuture::from(promise).await.unwrap_or(JsValue::NULL)
                    } else {
                        JsValue::NULL
                    }
                } else {
                    dest
                };
                if !resolved.is_null() && !resolved.is_undefined() {
                    let first = Reflect::get(&resolved, &JsValue::from_f64(0.0)).unwrap_or(JsValue::UNDEFINED);
                    if let Some(n) = first.as_f64() {
                        target = n as i32;
                    } else if !first.is_undefined() && !first.is_null() {
                        if let Ok(p) = call1(doc, "getPageIndex", &first)
                            && let Ok(promise) = p.dyn_into::<Promise>()
                            && let Ok(idx) = JsFuture::from(promise).await
                            && let Some(n) = idx.as_f64()
                        {
                            target = n as i32;
                        }
                    }
                }
            }
            let title = Reflect::get(&item, &JsValue::from_str("title"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            let trimmed = title.trim();
            if !trimmed.is_empty() {
                let label: String = trimmed.chars().take(200).collect();
                out.push(PdfOutlineEntry { label, page: target, depth: depth.min(6) });
            }
            if let Ok(children) = Reflect::get(&item, &JsValue::from_str("items")) {
                walk_outline(doc, children, depth + 1, out).await;
            }
        }
    })
}

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
}

impl PdfHandle {
    pub fn go_to(&self, page: usize, fraction: f64) {
        self.go_to.run((page, fraction));
    }
    pub fn step(&self, direction: i32) {
        self.step.run(direction);
    }
    pub fn zoom(&self, change: PdfZoomChange) {
        self.zoom.run(change);
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Layout {
    scale: f64,
    tops: Vec<f64>,
    total: f64,
}

/// A PDF drawn with pdf.js: pages are laid out at their real proportions in one scrolling
/// column, and only pages near the viewport are drawn on canvases.
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
) -> impl IntoView {
    let scroller = NodeRef::<html::Div>::new();
    let pages_node = NodeRef::<html::Div>::new();
    let doc: RwSignal<Option<SendWrapper<JsValue>>> = RwSignal::new(None);
    let doc_id = RwSignal::new(0u32);
    let sizes: RwSignal<Vec<(f64, f64)>> = RwSignal::new(Vec::new());
    let width = RwSignal::new(0.0_f64);
    let height = RwSignal::new(0.0_f64);
    let zoom = RwSignal::new(PdfZoom::Width);
    let failed = RwSignal::new(String::new());
    let visible = RwSignal::new((0usize, 2usize));

    let start = StoredValue::new((page, fraction));
    let restored = StoredValue::new(false);
    let kept = StoredValue::new((0usize, 0.0_f64));

    // Load PDF whenever `url` changes; cancel & destroy previous task on re-run or cleanup.
    let active_task: StoredValue<Option<SendWrapper<(Rc<Cell<bool>>, Option<JsValue>)>>> = StoredValue::new(None);
    let destroy_active = move || {
        active_task.update_value(|slot| {
            if let Some(prev) = slot.take() {
                let (alive, task) = prev.take();
                alive.set(false);
                if let Some(t) = task {
                    call_method(&t, "destroy");
                }
            }
        });
    };
    on_cleanup(move || destroy_active());

    Effect::new(move |_| {
        let current_url = url.get();
        destroy_active();
        failed.set(String::new());
        doc.set(None);
        sizes.set(Vec::new());
        restored.set_value(false);

        let alive = Rc::new(Cell::new(true));
        active_task.set_value(Some(SendWrapper::new((alive.clone(), None))));
        let alive_for_task = alive.clone();

        spawn_local(async move {
            let module = match load_pdfjs().await {
                Ok(m) => m,
                Err(_) => {
                    if alive_for_task.get() {
                        let msg = "The PDF could not be loaded.".to_owned();
                        failed.set(msg.clone());
                        if let Some(cb) = on_error {
                            cb.run(msg);
                        }
                    }
                    return;
                }
            };
            if !alive_for_task.get() {
                return;
            }
            let opts = Object::new();
            let _ = Reflect::set(&opts, &JsValue::from_str("url"), &JsValue::from_str(&current_url));
            let _ = Reflect::set(&opts, &JsValue::from_str("cMapUrl"), &JsValue::from_str(&format!("{ASSETS}cmaps/")));
            let _ = Reflect::set(&opts, &JsValue::from_str("cMapPacked"), &JsValue::from_bool(true));
            let _ = Reflect::set(&opts, &JsValue::from_str("standardFontDataUrl"), &JsValue::from_str(&format!("{ASSETS}standard_fonts/")));
            let _ = Reflect::set(&opts, &JsValue::from_str("wasmUrl"), &JsValue::from_str(&format!("{ASSETS}wasm/")));
            let _ = Reflect::set(&opts, &JsValue::from_str("iccUrl"), &JsValue::from_str(&format!("{ASSETS}iccs/")));
            let _ = Reflect::set(&opts, &JsValue::from_str("enableXfa"), &JsValue::from_bool(false));
            let _ = Reflect::set(&opts, &JsValue::from_str("isOffscreenCanvasSupported"), &JsValue::from_bool(true));

            let task = match call1(&module, "getDocument", &opts) {
                Ok(t) => t,
                Err(_) => {
                    if alive_for_task.get() {
                        let msg = "The PDF could not be loaded.".to_owned();
                        failed.set(msg.clone());
                        if let Some(cb) = on_error {
                            cb.run(msg);
                        }
                    }
                    return;
                }
            };
            if !alive_for_task.get() {
                call_method(&task, "destroy");
                return;
            }
            active_task.set_value(Some(SendWrapper::new((alive_for_task.clone(), Some(task.clone())))));

            let promise = match Reflect::get(&task, &JsValue::from_str("promise")).and_then(|p| p.dyn_into::<Promise>()) {
                Ok(p) => p,
                Err(_) => {
                    if alive_for_task.get() {
                        let msg = "The PDF could not be loaded.".to_owned();
                        failed.set(msg.clone());
                        if let Some(cb) = on_error {
                            cb.run(msg);
                        }
                    }
                    return;
                }
            };

            match JsFuture::from(promise).await {
                Ok(value) => {
                    if !alive_for_task.get() {
                        return;
                    }
                    let num_pages = Reflect::get(&value, &JsValue::from_str("numPages"))
                        .ok()
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0) as usize;
                    if num_pages == 0 {
                        let msg = "This file is not a readable PDF.".to_owned();
                        failed.set(msg.clone());
                        if let Some(cb) = on_error {
                            cb.run(msg);
                        }
                        return;
                    }
                    let first = match get_page_size(&value, 1).await {
                        Ok(s) => s,
                        Err(_) => {
                            if alive_for_task.get() {
                                let msg = "The PDF could not be loaded.".to_owned();
                                failed.set(msg.clone());
                                if let Some(cb) = on_error {
                                    cb.run(msg);
                                }
                            }
                            return;
                        }
                    };
                    if !alive_for_task.get() {
                        return;
                    }
                    let mut list = vec![first; num_pages];
                    sizes.set(list.clone());
                    doc.set(Some(SendWrapper::new(value.clone())));
                    doc_id.update(|n| *n += 1);

                    for i in 2..=num_pages {
                        if !alive_for_task.get() {
                            return;
                        }
                        if let Ok(v) = get_page_size(&value, i).await {
                            if (v.0 - first.0).abs() > f64::EPSILON || (v.1 - first.1).abs() > f64::EPSILON {
                                list[i - 1] = v;
                                if i % 25 == 0 || i == num_pages {
                                    sizes.set(list.clone());
                                }
                            }
                        }
                    }
                    if !alive_for_task.get() {
                        return;
                    }
                    sizes.set(list);

                    let mut outline = Vec::new();
                    if let Ok(p) = call0(&value, "getOutline")
                        && let Ok(promise) = p.dyn_into::<Promise>()
                        && let Ok(items) = JsFuture::from(promise).await
                    {
                        walk_outline(&value, items, 0, &mut outline).await;
                    }
                    if alive_for_task.get()
                        && let Some(cb) = on_ready
                    {
                        cb.run(PdfReady { pages: num_pages, outline });
                    }
                }
                Err(cause) => {
                    if !alive_for_task.get() {
                        return;
                    }
                    let name = Reflect::get(&cause, &JsValue::from_str("name"))
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_default();
                    let msg = match name.as_str() {
                        "PasswordException" => "This PDF is protected with a password. Download it to open it in another app.",
                        "InvalidPDFException" => "This file is not a readable PDF.",
                        _ => "The PDF could not be loaded.",
                    }
                    .to_owned();
                    failed.set(msg.clone());
                    if let Some(cb) = on_error {
                        cb.run(msg);
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
            width.set(node_for_cb.client_width() as f64);
            height.set(node_for_cb.client_height() as f64);
        });
        let observer = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()).expect("ResizeObserver");
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
        let w = width.get();
        let h = height.get();
        let z = zoom.get();
        let widest = s.iter().map(|(pw, _)| *pw).fold(1.0_f64, f64::max);
        let tallest = s.iter().map(|(_, ph)| *ph).fold(1.0_f64, f64::max);
        let scale = match z {
            PdfZoom::Scale(val) => val,
            PdfZoom::Width => ((w - PAD * 2.0) / widest).max(0.2),
            PdfZoom::Page => (((w - PAD * 2.0) / widest).min((h - PAD * 2.0) / tallest)).max(0.2),
        };
        let mut tops = Vec::with_capacity(s.len());
        let mut total = PAD;
        for (_, ph) in &s {
            tops.push(total);
            total += *ph * scale + GAP;
        }
        Layout { scale, tops, total }
    });

    // Set `--pdf-height` via CSSOM (no inline style attribute because of CSP).
    Effect::new(move |_| {
        let total = layout.with(|l| l.total);
        if let Some(node) = pages_node.get() {
            let el: &web_sys::HtmlElement = &node;
            let _ = el.style().set_property("--pdf-height", &format!("{}px", total.round() as i64));
        }
    });

    let compute_position = move || -> (usize, f64, usize) {
        let Some(node) = scroller.get_untracked() else {
            return (0, 0.0, 0);
        };
        let s = sizes.get_untracked();
        let l = layout.get_untracked();
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
        let Some(node) = scroller.get_untracked() else { return };
        let s = sizes.get_untracked();
        let l = layout.get_untracked();
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
        let _ = doc_id.get();
        let w = width.get();
        let l = layout.get();
        if doc.with_untracked(Option::is_none) || w <= 0.0 || l.tops.is_empty() {
            return;
        }
        if !restored.get_value() {
            restored.set_value(true);
            let (sp, sf) = start.get_value();
            go_to_fn(sp, sf);
            let (p, f, count) = compute_position();
            kept.set_value((p, f));
            if let Some(cb) = on_position {
                cb.run((p, f, count));
            }
        } else {
            let (kp, kf) = kept.get_value();
            go_to_fn(kp, kf);
        }
    });

    // Update visible window and report scroll position (rAF-throttled).
    let update_visible = move || {
        let Some(node) = scroller.get_untracked() else { return };
        let l = layout.get_untracked();
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
        if visible.get_untracked() != (a, b) {
            visible.set((a, b));
        }
    };

    Effect::new(move |_| {
        let _ = layout.get();
        let _ = height.get();
        update_visible();
    });

    // Attach passive scroll listener on scroller.
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
                if !restored.get_value() {
                    return;
                }
                let (p, f, count) = compute_position();
                kept.set_value((p, f));
                if let Some(cb) = on_position {
                    cb.run((p, f, count));
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

    // Expose PdfHandle if caller wants imperative control.
    if let Some(slot) = handle {
        let change_zoom = move |next: PdfZoom| {
            zoom.set(next);
            if let Some(cb) = on_zoom {
                cb.run(next);
            }
        };
        slot.set(Some(PdfHandle {
            go_to: Callback::new(move |(p, f)| go_to_fn(p, f)),
            step: Callback::new(move |direction: i32| {
                if let Some(node) = scroller.get_untracked() {
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
                    let s = layout.with_untracked(|l| l.scale);
                    change_zoom(PdfZoom::Scale((s * 1.25).clamp(0.25, 5.0)));
                }
                PdfZoomChange::Out => {
                    let s = layout.with_untracked(|l| l.scale);
                    change_zoom(PdfZoom::Scale((s * 0.8).clamp(0.25, 5.0)));
                }
            }),
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

    view! {
        <div
            node_ref=scroller
            class=format!("pdf-view {class_name}")
            tabindex="0"
            role="document"
            aria-label=move || label.get()
            aria-busy=move || (doc.with(Option::is_none) && failed.with(String::is_empty)).to_string()
        >
            {move || {
                let err = failed.get();
                if !err.is_empty() {
                    view! { <div class="pdf-view-message" role="alert">{err}</div> }.into_any()
                } else if doc.with(Option::is_none) {
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
                                            doc=doc
                                            doc_id=doc_id
                                            sizes=sizes
                                            layout=layout
                                            index=index
                                        />
                                    }
                                }
                            />
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
    task: Rc<RefCell<Option<JsValue>>>,
    page: Rc<RefCell<Option<JsValue>>>,
}

impl RenderGuard {
    fn cancel(&self) {
        self.alive.set(false);
        if let Some(t) = self.task.borrow_mut().take() {
            call_method(&t, "cancel");
        }
        if let Some(p) = self.page.borrow_mut().take() {
            call_method(&p, "cleanup");
        }
    }
}

/// One page box; its canvas is drawn at the screen's pixel density.
#[component]
fn PdfPage(
    doc: RwSignal<Option<SendWrapper<JsValue>>>,
    doc_id: RwSignal<u32>,
    sizes: RwSignal<Vec<(f64, f64)>>,
    layout: Memo<Layout>,
    index: usize,
) -> impl IntoView {
    let box_ref = NodeRef::<html::Div>::new();
    let canvas_ref = NodeRef::<html::Canvas>::new();
    let drawn = RwSignal::new(false);

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
    });

    // Render page onto canvas after 60ms settle; cancel previous render on scale/doc change or unmount.
    let guard: StoredValue<Option<SendWrapper<RenderGuard>>> = StoredValue::new(None);
    let cancel_current = move || {
        guard.update_value(|slot| {
            if let Some(prev) = slot.take() {
                prev.take().cancel();
            }
        });
    };
    on_cleanup(move || cancel_current());

    Effect::new(move |_| {
        let _ = doc_id.get();
        let scale = layout.with(|l| l.scale);
        let Some(doc_val) = doc.with_untracked(|d| d.as_ref().map(|w| (**w).clone())) else {
            return;
        };
        cancel_current();
        let alive = Rc::new(Cell::new(true));
        let task_slot: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));
        let page_slot: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));
        guard.set_value(Some(SendWrapper::new(RenderGuard {
            alive: alive.clone(),
            task: task_slot.clone(),
            page: page_slot.clone(),
        })));

        spawn_local(async move {
            crate::sleep(60).await;
            if !alive.get() {
                return;
            }
            let page_promise = match call1(&doc_val, "getPage", &JsValue::from_f64((index + 1) as f64))
                .and_then(|p| p.dyn_into::<Promise>())
            {
                Ok(p) => p,
                Err(_) => return,
            };
            let page = match JsFuture::from(page_promise).await {
                Ok(p) => p,
                Err(_) => return,
            };
            if !alive.get() {
                call_method(&page, "cleanup");
                return;
            }
            let Some(target) = canvas_ref.get_untracked() else {
                call_method(&page, "cleanup");
                return;
            };
            *page_slot.borrow_mut() = Some(page.clone());

            let dpr = window().device_pixel_ratio();
            let ratio = if dpr > 0.0 { dpr.min(2.0) } else { 1.0 };
            let vp_opts = Object::new();
            let _ = Reflect::set(&vp_opts, &JsValue::from_str("scale"), &JsValue::from_f64(scale * ratio));
            let viewport = match call1(&page, "getViewport", &vp_opts) {
                Ok(v) => v,
                Err(_) => return,
            };
            let vw = Reflect::get(&viewport, &JsValue::from_str("width"))
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let vh = Reflect::get(&viewport, &JsValue::from_str("height"))
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            target.set_width(vw.floor().max(1.0) as u32);
            target.set_height(vh.floor().max(1.0) as u32);

            let render_opts = Object::new();
            let _ = Reflect::set(&render_opts, &JsValue::from_str("canvas"), target.as_ref());
            let _ = Reflect::set(&render_opts, &JsValue::from_str("viewport"), &viewport);
            let task = match call1(&page, "render", &render_opts) {
                Ok(t) => t,
                Err(_) => return,
            };
            *task_slot.borrow_mut() = Some(task.clone());
            if let Ok(p) = Reflect::get(&task, &JsValue::from_str("promise"))
                && let Ok(promise) = p.dyn_into::<Promise>()
                && JsFuture::from(promise).await.is_ok()
                && alive.get()
            {
                let _ = drawn.try_set(true);
            }
        });
    });

    view! {
        <div
            node_ref=box_ref
            class=move || if drawn.get() { "pdf-view-page is-drawn" } else { "pdf-view-page" }
            aria-label=format!("Page {}", index + 1)
            role="img"
        >
            <canvas node_ref=canvas_ref/>
        </div>
    }
}
