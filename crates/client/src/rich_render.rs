//! Lazy loader and hydrator for KaTeX (MathML) and Mermaid diagrams inside Markdown previews.
//!
//! `crates/client/render/{katex,mermaid}.js` are bundled by `cargo xtask` with esbuild into
//! separate content-hashed modules (`katex-render.<hash>.js` and `mermaid-render.<hash>.js`),
//! whose URLs sit on `<html>` as `data-katex-render` and `data-mermaid-render`. Neither bundle
//! is fetched until a mounted `<Markdown/>` preview actually contains math or a Mermaid block.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::LocalKey;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::ui::document;

// A static module snippet, so dynamic imports satisfy `script-src 'self'`.
#[wasm_bindgen(inline_js = "export function import_rich_renderer(url) { return import(url); }")]
extern "C" {
    fn import_rich_renderer(url: &str) -> js_sys::Promise;
}

thread_local! {
    static KATEX: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    static MERMAID: RefCell<Option<JsValue>> = const { RefCell::new(None) };
}

async fn load_module(cache: &'static LocalKey<RefCell<Option<JsValue>>>, attr: &str) -> Result<JsValue, String> {
    if let Some(module) = cache.with(|m| m.borrow().clone()) {
        return Ok(module);
    }
    let url = document()
        .document_element()
        .and_then(|root| root.get_attribute(attr))
        .ok_or_else(|| format!("Renderer URL attribute {attr} is missing from this build."))?;
    let module = JsFuture::from(import_rich_renderer(&url))
        .await
        .map_err(|_| format!("Could not load renderer module {url}."))?;
    cache.with(|m| *m.borrow_mut() = Some(module.clone()));
    Ok(module)
}

fn own_placeholders(root: &web_sys::Element, selector: &str) -> Vec<web_sys::Element> {
    let Ok(list) = root.query_selector_all(selector) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(list.length() as usize);
    for i in 0..list.length() {
        let Some(el) = list.item(i).and_then(|n| n.dyn_into::<web_sys::Element>().ok()) else {
            continue;
        };
        // Skip placeholders inside nested embeds (`note_embed`), which hydrate themselves.
        if el.closest(".markdown-preview").ok().flatten().as_ref() == Some(root) {
            out.push(el);
        }
    }
    out
}

fn mark_error(root: &web_sys::Element, selector: &str) {
    for el in own_placeholders(root, selector) {
        let list = el.class_list();
        if !list.contains("is-rendered") {
            let _ = list.add_1("is-error");
        }
    }
}

async fn hydrate_math(root: NodeRef<html::Div>, alive: Arc<AtomicBool>) {
    let module = load_module(&KATEX, "data-katex-render").await;
    if !alive.load(Ordering::Relaxed) {
        return;
    }
    let Some(container) = root.try_get_untracked().flatten() else {
        return;
    };
    let root_el: web_sys::Element = container.unchecked_into();
    if !root_el.is_connected() {
        return;
    }
    let Ok(module) = module else {
        mark_error(&root_el, ".math[data-tex]");
        return;
    };
    let Ok(render) = js_sys::Reflect::get(&module, &JsValue::from_str("render"))
        .and_then(|value| value.dyn_into::<js_sys::Function>())
    else {
        mark_error(&root_el, ".math[data-tex]");
        return;
    };

    for el in own_placeholders(&root_el, ".math[data-tex]") {
        if !alive.load(Ordering::Relaxed) || !el.is_connected() {
            return;
        }
        let list = el.class_list();
        if list.contains("is-rendered") || list.contains("is-error") {
            continue;
        }
        let Some(tex) = el.get_attribute("data-tex") else {
            continue;
        };
        let display = el.has_attribute("data-display");
        match render.call3(&JsValue::NULL, &el, &JsValue::from_str(&tex), &JsValue::from_bool(display)) {
            Ok(_) => {
                let _ = list.remove_1("is-error");
                let _ = list.add_1("is-rendered");
            }
            Err(_) => {
                let _ = list.remove_1("is-rendered");
                let _ = list.add_1("is-error");
            }
        }
    }
}

async fn hydrate_mermaid(root: NodeRef<html::Div>, alive: Arc<AtomicBool>) {
    let module = load_module(&MERMAID, "data-mermaid-render").await;
    if !alive.load(Ordering::Relaxed) {
        return;
    }
    let Some(container) = root.try_get_untracked().flatten() else {
        return;
    };
    let root_el: web_sys::Element = container.unchecked_into();
    if !root_el.is_connected() {
        return;
    }
    let Ok(module) = module else {
        mark_error(&root_el, "figure.mermaid[data-source]");
        return;
    };
    let Ok(render) = js_sys::Reflect::get(&module, &JsValue::from_str("render"))
        .and_then(|value| value.dyn_into::<js_sys::Function>())
    else {
        mark_error(&root_el, "figure.mermaid[data-source]");
        return;
    };

    for el in own_placeholders(&root_el, "figure.mermaid[data-source]") {
        if !alive.load(Ordering::Relaxed) || root.try_get_untracked().flatten().is_none() || !el.is_connected() {
            return;
        }
        let list = el.class_list();
        if list.contains("is-rendered") || list.contains("is-error") {
            continue;
        }
        let Some(source) = el.get_attribute("data-source") else {
            continue;
        };
        let promise = render
            .call2(&JsValue::NULL, &el, &JsValue::from_str(&source))
            .and_then(|value| value.dyn_into::<js_sys::Promise>());
        let outcome = match promise {
            Ok(p) => JsFuture::from(p).await,
            Err(err) => Err(err),
        };
        if !alive.load(Ordering::Relaxed) || root.try_get_untracked().flatten().is_none() || !el.is_connected() {
            return;
        }
        match outcome {
            Ok(_) => {
                let _ = list.remove_1("is-error");
                let _ = list.add_1("is-rendered");
            }
            Err(_) => {
                let _ = list.remove_1("is-rendered");
                let _ = list.add_1("is-error");
            }
        }
    }
}

/// Hydrates math and Mermaid placeholders inside `root` after mount. Owner-scoped: cancels
/// in-flight hydration when the preview unmounts and accesses `root` via `try_` methods.
pub fn hydrate(root: NodeRef<html::Div>, has_math: bool, has_mermaid: bool) {
    if !has_math && !has_mermaid {
        return;
    }
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }
    Effect::new(move |started: Option<bool>| {
        if started == Some(true) {
            return true;
        }
        if root.try_get().flatten().is_none() {
            return false;
        }
        if has_math {
            let alive = alive.clone();
            spawn_local(async move {
                hydrate_math(root, alive).await;
            });
        }
        if has_mermaid {
            let alive = alive.clone();
            spawn_local(async move {
                hydrate_mermaid(root, alive).await;
            });
        }
        true
    });
}
