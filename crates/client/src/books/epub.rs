//! EPUB chapter viewer (`next/client/books/epub.tsx`): a same-origin iframe whose document
//! has no script access (`script-src 'none'`). The reader styles the chapter (theme, font,
//! size, spacing) and, in Pages layout, splits it into screen-wide CSS columns scrolled one
//! page at a time.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::{Object, Promise, Reflect};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::SpineItem;
use crate::ui::window;

#[wasm_bindgen(inline_js = "export function dispatch_key(t, k) { t.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true })); }")]
extern "C" {
    fn dispatch_key(t: &web_sys::EventTarget, k: &str);
}

const SERIF: &str = "\"Iowan Old Style\",\"Charter\",\"Literata\",\"Georgia\",serif";
const SANS: &str = "system-ui,-apple-system,\"Segoe UI\",\"Noto Sans\",sans-serif";

#[derive(Clone, Debug, PartialEq)]
pub struct ReaderLook {
    pub background: String,
    pub color: String,
    pub link: String,
    pub dark: bool,
    pub font: String,
    pub size: u32,
    pub line_height: f64,
    pub layout: String,
}

#[derive(Clone)]
pub struct EpubHandle {
    next: Callback<()>,
    previous: Callback<()>,
    go_to: Callback<(usize, f64, Option<String>)>,
}

impl EpubHandle {
    pub fn next(&self) {
        self.next.run(());
    }
    pub fn previous(&self) {
        self.previous.run(());
    }
    pub fn go_to(&self, spine: usize, fraction: f64, anchor: Option<String>) {
        self.go_to.run((spine, fraction, anchor));
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Target {
    spine: usize,
    fraction: f64,
    anchor: Option<String>,
    html: bool,
    load: u64,
}

struct DocListener {
    target: web_sys::EventTarget,
    name: &'static str,
    callback: Closure<dyn FnMut(web_sys::Event)>,
}

impl DocListener {
    fn new(
        target: &web_sys::EventTarget,
        name: &'static str,
        passive: Option<bool>,
        once: bool,
        f: impl FnMut(web_sys::Event) + 'static,
    ) -> Self {
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(f);
        let opts = web_sys::AddEventListenerOptions::new();
        if let Some(p) = passive {
            opts.set_passive(p);
        }
        if once {
            opts.set_once(true);
        }
        let _ = target.add_event_listener_with_callback_and_add_event_listener_options(
            name,
            callback.as_ref().unchecked_ref(),
            &opts,
        );
        Self { target: target.clone(), name, callback }
    }
}

impl Drop for DocListener {
    fn drop(&mut self) {
        let _ = self.target.remove_event_listener_with_callback(self.name, self.callback.as_ref().unchecked_ref());
    }
}

fn encode_path(href: &str) -> String {
    href.split('/')
        .map(|seg| js_sys::encode_uri_component(seg).as_string().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("/")
}

fn scroller(doc: &web_sys::Document) -> Option<web_sys::Element> {
    doc.scrolling_element().or_else(|| doc.document_element())
}

fn rtl_flow(doc: &web_sys::Document) -> bool {
    let el = doc
        .body()
        .map(JsCast::unchecked_into::<web_sys::Element>)
        .or_else(|| doc.document_element());
    let Some(el) = el else { return false };
    let view = doc.default_view().unwrap_or_else(window);
    view.get_computed_style(&el)
        .ok()
        .flatten()
        .and_then(|s| s.get_property_value("direction").ok())
        .is_some_and(|d| d == "rtl")
}

struct EpubState {
    frame: NodeRef<html::Iframe>,
    spine_len: usize,
    look: Signal<ReaderLook>,
    right_to_left: Signal<bool>,
    target: RwSignal<Target>,
    page_index: Cell<usize>,
    page_count: Cell<usize>,
    page_width: Cell<f64>,
    current_spine: Cell<usize>,
    current_fraction: Cell<f64>,
    turning: Cell<f64>,
    load_seq: Cell<u64>,
    ready_doc: RefCell<Option<web_sys::Document>>,
    doc_listeners: RefCell<Vec<DocListener>>,
    on_position: Callback<(usize, f64)>,
    on_link: Callback<String>,
    on_error: Callback<String>,
}

impl EpubState {
    fn next_load(&self) -> u64 {
        let next = self.load_seq.get() + 1;
        self.load_seq.set(next);
        next
    }

    fn paged(&self) -> bool {
        self.look.with_untracked(|l| l.layout == "pages")
    }

    fn doc(&self) -> Option<web_sys::Document> {
        let value = self.frame.get_untracked()?.content_document()?;
        let ready = self.ready_doc.borrow();
        let expected = ready.as_ref()?;
        Object::is(value.as_ref(), expected.as_ref()).then_some(value)
    }

    fn is_ready(&self, doc: &web_sys::Document) -> bool {
        self.ready_doc
            .borrow()
            .as_ref()
            .is_some_and(|r| Object::is(doc.as_ref(), r.as_ref()))
    }

    fn report(&self) {
        { self.on_position.try_run((self.current_spine.get(), self.current_fraction.get())); };
    }

    /// Puts the reader's stylesheet into the chapter, replacing an earlier one.
    fn style(&self, doc: &web_sys::Document) {
        let Some(frame_node) = self.frame.get_untracked() else { return };
        let l = self.look.get_untracked();
        let paged = l.layout == "pages";
        let width = frame_node.client_width() as f64;
        let height = frame_node.client_height() as f64;
        let columns: f64 = if paged && width >= 1100.0 { 2.0 } else { 1.0 };
        let widest = ((l.size as f64) * if columns == 2.0 { 30.0 } else { 36.0 }).round();
        let min_pad: f64 = if width < 600.0 { 20.0 } else if width < 1000.0 { 40.0 } else { 56.0 };
        let pad_x = min_pad.max(((width / columns - widest) / 2.0).floor()) as i64;
        let pad_y: i64 = if width < 600.0 { 24 } else { 40 };
        let gap = pad_x * 2;
        let column = if paged { (width / columns - gap as f64).floor().max(1.0) as i64 } else { 0 };
        let font_decl = match l.font.as_str() {
            "book" => String::new(),
            "sans" => format!("font-family:{SANS}!important;"),
            _ => format!("font-family:{SERIF}!important;"),
        };
        let font_inherit = if l.font == "book" {
            ""
        } else {
            "body :not(pre,code,kbd,samp,pre *,code *){font-family:inherit!important}"
        };
        let scheme = if l.dark { "dark" } else { "light" };
        let max_media_h = ((height as i64) - pad_y * 2).max(1);
        let layout_css = if paged {
            format!(
                "html{{height:100%!important;overflow:hidden!important}}body{{margin:0!important;box-sizing:border-box!important;height:100vh!important;padding:{pad_y}px {pad_x}px!important;column-width:{column}px!important;column-gap:{gap}px!important;column-fill:auto!important;max-width:none!important}}img,svg,video{{max-height:{max_media_h}px!important}}"
            )
        } else {
            format!(
                "html{{overflow-y:auto!important;overflow-x:hidden!important;height:auto!important}}body{{margin:0 auto!important;max-width:42em!important;padding:32px {pad_x}px 40vh!important;box-sizing:border-box!important}}"
            )
        };
        let css = [
            format!(
                "html{{background:{}!important;color:{}!important;color-scheme:{scheme};font-size:{}px!important;-webkit-text-size-adjust:none;text-size-adjust:none}}",
                l.background, l.color, l.size
            ),
            format!(
                "body{{background:transparent!important;color:inherit!important;line-height:{}!important;{font_decl}overflow-wrap:break-word}}",
                l.line_height
            ),
            font_inherit.to_owned(),
            "body *{color:inherit!important;background-color:transparent!important;border-color:color-mix(in srgb,currentColor 25%,transparent)!important}".to_owned(),
            format!("a,a *{{color:{}!important}}", l.link),
            "img,svg,video,canvas{max-width:100%!important;height:auto;object-fit:contain;break-inside:avoid}".to_owned(),
            format!("::selection{{background:color-mix(in srgb,{} 30%,transparent)}}", l.link),
            layout_css,
        ]
        .join("\n");

        let node = match doc.get_element_by_id("dar-reader-style") {
            Some(existing) => existing,
            None => {
                let Ok(created) = doc.create_element_ns(Some("http://www.w3.org/1999/xhtml"), "style") else {
                    return;
                };
                created.set_id("dar-reader-style");
                if let Some(parent) = doc
                    .query_selector("head")
                    .ok()
                    .flatten()
                    .or_else(|| doc.document_element())
                {
                    let _ = parent.append_child(&created);
                }
                created
            }
        };
        node.set_text_content(Some(&css));
    }

    fn measure(&self, doc: &web_sys::Document) {
        let Some(frame_node) = self.frame.get_untracked() else { return };
        let Some(el) = scroller(doc) else { return };
        let width = (frame_node.client_width() as f64).max(1.0);
        self.page_width.set(width);
        let count = ((((el.scroll_width() as f64) - 4.0) / width).ceil() as usize).max(1);
        self.page_count.set(count);
    }

    fn show(&self, doc: &web_sys::Document, index: usize) {
        let Some(el) = scroller(doc) else { return };
        let count = self.page_count.get().max(1);
        let clamped = index.min(count - 1);
        self.page_index.set(clamped);
        let sign = if rtl_flow(doc) { -1.0 } else { 1.0 };
        el.set_scroll_left((sign * (clamped as f64) * self.page_width.get()) as i32);
        let frac = if count > 1 { clamped as f64 / (count - 1) as f64 } else { 0.0 };
        self.current_fraction.set(frac);
    }

    /// Moves to a fraction of the chapter, or to an element by ID.
    fn place(&self, doc: &web_sys::Document, fraction: f64, anchor: Option<&str>) {
        let Some(el) = scroller(doc) else { return };
        if self.paged() {
            self.measure(doc);
            if let Some(found) = anchor.and_then(|id| doc.get_element_by_id(id)) {
                let rect = found.get_bounding_client_rect();
                let sign = if rtl_flow(doc) { -1.0 } else { 1.0 };
                let left = (rect.left() + sign * (el.scroll_left() as f64)).abs();
                let idx = (left / self.page_width.get().max(1.0)).floor().max(0.0) as usize;
                self.show(doc, idx);
            } else {
                let count = self.page_count.get().max(1);
                let idx = (fraction * ((count - 1) as f64)).round().max(0.0) as usize;
                self.show(doc, idx);
            }
        } else {
            if let Some(found) = anchor.and_then(|id| doc.get_element_by_id(id)) {
                let opts = web_sys::ScrollIntoViewOptions::new();
                opts.set_block(web_sys::ScrollLogicalPosition::Start);
                found.scroll_into_view_with_scroll_into_view_options(&opts);
            } else {
                let max_scroll = (el.scroll_height() - el.client_height()).max(0) as f64;
                el.set_scroll_top((fraction * max_scroll) as i32);
            }
            let max_scroll = (el.scroll_height() - el.client_height()).max(1) as f64;
            self.current_fraction.set(((el.scroll_top() as f64) / max_scroll).clamp(0.0, 1.0));
        }
    }

    fn go(&self, spine_index: isize, fraction: f64, anchor: Option<String>) {
        if spine_index < 0 || (spine_index as usize) >= self.spine_len {
            return;
        }
        let spine_index = spine_index as usize;
        if spine_index == self.current_spine.get()
            && let Some(doc) = self.doc()
            && doc.body().is_some()
        {
            self.place(&doc, fraction, anchor.as_deref());
            self.report();
            return;
        }
        self.current_spine.set(spine_index);
        self.current_fraction.set(fraction);
        let load = self.next_load();
        self.target.set(Target { spine: spine_index, fraction, anchor, html: false, load });
    }

    fn step(&self, direction: i32) {
        let Some(doc) = self.doc() else { return };
        if self.paged() {
            let idx = self.page_index.get() as isize;
            let next = idx + direction as isize;
            if next >= 0 && (next as usize) < self.page_count.get() {
                self.show(&doc, next as usize);
                self.report();
                return;
            }
        } else if let Some(el) = scroller(&doc) {
            let before = el.scroll_top() as f64;
            let delta = (direction as f64) * (el.client_height() as f64) * 0.9;
            el.set_scroll_top((before + delta) as i32);
            if ((el.scroll_top() as f64) - before).abs() > 2.0 {
                let max_scroll = (el.scroll_height() - el.client_height()).max(1) as f64;
                self.current_fraction.set(((el.scroll_top() as f64) / max_scroll).clamp(0.0, 1.0));
                self.report();
                return;
            }
        }
        let next_spine = self.current_spine.get() as isize + direction as isize;
        self.go(next_spine, if direction > 0 { 0.0 } else { 1.0 }, None);
    }

    fn keys(&self, event: &web_sys::KeyboardEvent) {
        if event.default_prevented() || event.alt_key() || event.ctrl_key() || event.meta_key() {
            return;
        }
        let key = event.key();
        let rtl = self.right_to_left.get_untracked();
        let paged = self.paged();
        let forward = ((key == "PageDown" || key == " ") && !event.shift_key())
            || key == if rtl { "ArrowLeft" } else { "ArrowRight" }
            || (paged && key == "ArrowDown");
        let back = key == "PageUp"
            || (key == " " && event.shift_key())
            || key == if rtl { "ArrowRight" } else { "ArrowLeft" }
            || (paged && key == "ArrowUp");
        if forward {
            event.prevent_default();
            self.step(1);
        } else if back {
            event.prevent_default();
            self.step(-1);
        } else if matches!(key.as_str(), "Escape" | "Home" | "End")
            && let Some(frame_node) = self.frame.get_untracked()
        {
            // Let the reader's own shortcuts (Escape closes) see keys pressed inside the chapter.
            dispatch_key(frame_node.as_ref(), &key);
        }
    }

    fn loaded(self: &Rc<Self>) {
        self.doc_listeners.borrow_mut().clear();
        let Some(frame_node) = self.frame.get_untracked() else { return };
        let Some(doc) = frame_node.content_document() else { return };
        if doc.url().ok().as_deref() == Some("about:blank") {
            return;
        }
        *self.ready_doc.borrow_mut() = Some(doc.clone());

        let t = self.target.get_untracked();
        let broken = doc
            .document_element()
            .is_some_and(|el| el.node_name() == "parsererror")
            || doc.query_selector("parsererror").ok().flatten().is_some();
        if broken && !t.html {
            let load = self.next_load();
            self.target.set(Target { html: true, load, ..t });
            return;
        }
        if doc.body().is_none() {
            { self.on_error.try_run("This chapter could not be read.".into()); };
            return;
        }

        self.style(&doc);
        self.place(&doc, t.fraction, t.anchor.as_deref());
        self.report();

        let mut listeners = Vec::new();

        // Images and fonts change the layout after load; keep the reader in the same place.
        if let Ok(images) = doc.query_selector_all("img") {
            for i in 0..images.length() {
                if let Some(node) = images.item(i)
                    && let Ok(img) = node.dyn_into::<web_sys::HtmlImageElement>()
                    && !img.complete()
                {
                    let state = self.clone();
                    let doc_c = doc.clone();
                    listeners.push(DocListener::new(img.as_ref(), "load", None, true, move |_| {
                        if state.is_ready(&doc_c) {
                            state.place(&doc_c, state.current_fraction.get(), None);
                        }
                    }));
                }
            }
        }
        if let Ok(fonts) = Reflect::get(&doc, &JsValue::from_str("fonts"))
            && !fonts.is_undefined()
            && !fonts.is_null()
            && let Ok(ready) = Reflect::get(&fonts, &JsValue::from_str("ready"))
            && let Ok(promise) = ready.dyn_into::<Promise>()
        {
            let state = self.clone();
            let doc_c = doc.clone();
            spawn_local(async move {
                let _ = JsFuture::from(promise).await;
                if state.is_ready(&doc_c) {
                    state.place(&doc_c, state.current_fraction.get(), None);
                }
            });
        }

        // Keyboard inside chapter document.
        {
            let state = self.clone();
            listeners.push(DocListener::new(doc.as_ref(), "keydown", None, false, move |event| {
                if let Ok(ke) = event.dyn_into::<web_sys::KeyboardEvent>() {
                    state.keys(&ke);
                }
            }));
        }

        // Clicks: links call on_link; outer 30% turns pages in paged mode.
        {
            let state = self.clone();
            let doc_c = doc.clone();
            listeners.push(DocListener::new(doc.as_ref(), "click", None, false, move |event| {
                let Ok(me) = event.dyn_into::<web_sys::MouseEvent>() else { return };
                if let Some(link) = me
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .and_then(|el| el.closest("a[href]").ok().flatten())
                {
                    me.prevent_default();
                    let href = link
                        .dyn_ref::<web_sys::HtmlAnchorElement>()
                        .map(|a| a.href())
                        .or_else(|| Reflect::get(&link, &JsValue::from_str("href")).ok().and_then(|v| v.as_string()))
                        .unwrap_or_default();
                    { state.on_link.try_run(href); };
                    return;
                }
                if !state.paged() {
                    return;
                }
                let has_selection = doc_c
                    .get_selection()
                    .ok()
                    .flatten()
                    .and_then(|s| s.to_string().as_string())
                    .is_some_and(|s| !s.is_empty());
                if has_selection {
                    return;
                }
                let Some(f) = state.frame.get_untracked() else { return };
                let width = (f.client_width() as f64).max(1.0);
                let x = (me.client_x() as f64) / width;
                let rtl = state.right_to_left.get_untracked();
                let forward = if rtl { x < 0.3 } else { x > 0.7 };
                let back = if rtl { x > 0.7 } else { x < 0.3 };
                if forward {
                    state.step(1);
                } else if back {
                    state.step(-1);
                }
            }));
        }

        // Touch swipe in paged mode.
        let swipe: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
        {
            let swipe = swipe.clone();
            listeners.push(DocListener::new(doc.as_ref(), "touchstart", Some(true), false, move |event| {
                if let Ok(te) = event.dyn_into::<web_sys::TouchEvent>()
                    && let Some(t) = te.touches().item(0)
                {
                    swipe.set(Some((t.client_x() as f64, t.client_y() as f64)));
                }
            }));
        }
        {
            let state = self.clone();
            let swipe = swipe.clone();
            listeners.push(DocListener::new(doc.as_ref(), "touchend", Some(true), false, move |event| {
                let Ok(te) = event.dyn_into::<web_sys::TouchEvent>() else { return };
                let Some(t) = te.changed_touches().item(0) else { return };
                let Some((sx, sy)) = swipe.take() else { return };
                if !state.paged() {
                    return;
                }
                let dx = (t.client_x() as f64) - sx;
                let dy = (t.client_y() as f64) - sy;
                if dx.abs() > 50.0 && dx.abs() > dy.abs() * 1.5 {
                    let rtl = state.right_to_left.get_untracked();
                    state.step(if (dx < 0.0) != rtl { 1 } else { -1 });
                }
            }));
        }

        // Wheel turns pages in paged mode (with 350ms cooldown).
        {
            let state = self.clone();
            listeners.push(DocListener::new(doc.as_ref(), "wheel", Some(false), false, move |event| {
                if !state.paged() {
                    return;
                }
                let Ok(we) = event.dyn_into::<web_sys::WheelEvent>() else { return };
                let now = js_sys::Date::now();
                if now - state.turning.get() < 350.0 {
                    return;
                }
                let dx = we.delta_x();
                let dy = we.delta_y();
                let delta = if dx.abs() > dy.abs() { dx } else { dy };
                if delta.abs() < 20.0 {
                    return;
                }
                state.turning.set(now);
                we.prevent_default();
                state.step(if delta > 0.0 { 1 } else { -1 });
            }));
        }

        // Scroll updates fraction in scroll layout.
        {
            let state = self.clone();
            let doc_c = doc.clone();
            listeners.push(DocListener::new(doc.as_ref(), "scroll", Some(true), false, move |_| {
                if state.paged() {
                    return;
                }
                if let Some(el) = scroller(&doc_c) {
                    let max_scroll = (el.scroll_height() - el.client_height()).max(1) as f64;
                    state.current_fraction.set(((el.scroll_top() as f64) / max_scroll).clamp(0.0, 1.0));
                    state.report();
                }
            }));
        }

        *self.doc_listeners.borrow_mut() = listeners;
    }
}

#[component]
pub fn EpubView(
    #[prop(into)] base: String,
    spine: Vec<SpineItem>,
    #[prop(into)] right_to_left: Signal<bool>,
    #[prop(into)] look: Signal<ReaderLook>,
    start_spine: usize,
    start_fraction: f64,
    #[prop(into)] label: Signal<String>,
    handle: RwSignal<Option<EpubHandle>>,
    on_position: Callback<(usize, f64)>,
    on_link: Callback<String>,
    on_error: Callback<String>,
) -> impl IntoView {
    let frame = NodeRef::<html::Iframe>::new();
    let target = RwSignal::new(Target {
        spine: start_spine,
        fraction: start_fraction,
        anchor: None,
        html: false,
        load: 0,
    });
    let state = Rc::new(EpubState {
        frame,
        spine_len: spine.len(),
        look,
        right_to_left,
        target,
        page_index: Cell::new(0),
        page_count: Cell::new(1),
        page_width: Cell::new(1.0),
        current_spine: Cell::new(start_spine),
        current_fraction: Cell::new(start_fraction),
        turning: Cell::new(0.0),
        load_seq: Cell::new(0),
        ready_doc: RefCell::new(None),
        doc_listeners: RefCell::new(Vec::new()),
        on_position,
        on_link,
        on_error,
    });
    let wrapped = SendWrapper::new(state);

    // Expose imperative EpubHandle.
    {
        let s1 = wrapped.clone();
        let s2 = wrapped.clone();
        let s3 = wrapped.clone();
        handle.set(Some(EpubHandle {
            next: Callback::new(move |_| s1.step(1)),
            previous: Callback::new(move |_| s2.step(-1)),
            go_to: Callback::new(move |(s, f, a)| s3.go(s as isize, f, a)),
        }));
    }
    {
        let cleanup_state = wrapped.clone();
        on_cleanup(move || {
            cleanup_state.doc_listeners.borrow_mut().clear();
            let _ = handle.try_set(None);
        });
    }

    // Theme, font, size, line-height or layout changes restyle in place.
    {
        let state = wrapped.clone();
        Effect::new(move |_| {
            let _ = look.get();
            if let Some(doc) = state.doc()
                && doc.body().is_some()
            {
                let frac = state.current_fraction.get();
                state.style(&doc);
                state.place(&doc, frac, None);
            }
        });
    }

    // Resizing re-measures and keeps the same reading place.
    {
        let state = wrapped.clone();
        Effect::new(move |_| {
            let Some(node) = frame.get() else { return };
            let last = Rc::new(RefCell::new(format!("{}x{}", node.client_width(), node.client_height())));
            let node_cb = node.clone();
            let state_cb = state.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                let size = format!("{}x{}", node_cb.client_width(), node_cb.client_height());
                if *last.borrow() == size {
                    return;
                }
                *last.borrow_mut() = size;
                if let Some(doc) = state_cb.doc()
                    && doc.body().is_some()
                {
                    let frac = state_cb.current_fraction.get();
                    state_cb.style(&doc);
                    state_cb.place(&doc, frac, None);
                }
            });
            let observer = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()).expect("ResizeObserver");
            observer.observe(&node);
            let kept = SendWrapper::new((observer, cb));
            on_cleanup(move || {
                let (observer, _cb) = kept.take();
                observer.disconnect();
            });
        });
    }

    let base = StoredValue::new(base);
    let spine = StoredValue::new(spine);
    let state_view = wrapped.clone();

    move || {
        let t = target.get();
        let href = spine.with_value(|s| s.get(t.spine).map(|i| i.href.clone()).unwrap_or_default());
        let src = format!(
            "{}{}{}",
            base.get_value(),
            encode_path(&href),
            if t.html { "?as=html" } else { "" }
        );
        let state_load = state_view.clone();
        let state_key = state_view.clone();
        view! {
            <iframe
                node_ref=frame
                class=move || if look.with(|l| l.layout == "pages") { "epub-view is-paged" } else { "epub-view" }
                src=src
                title=move || label.get()
                sandbox="allow-same-origin allow-scripts"
                on:load=move |_| state_load.loaded()
                on:keydown=move |e| state_key.keys(&e)
            />
        }
    }
}
