//! The Notes editor's bridge to CodeMirror 6 (`crates/client/editor/notes-editor.js`, bundled
//! by `cargo xtask` as `notes-editor.<hash>.js`, its URL in `data-notes-editor` on `<html>`).
//! The module is imported the first time a note opens; `create` builds an editor in a host
//! element and calls back into Rust for every change, key, pasted or dropped file and followed
//! link. Positions are UTF-16 code units, as the editor counts them.

use std::cell::RefCell;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::markdown::split_fragment;
use crate::ui::document;

// A static module snippet, so the import satisfies `script-src 'self'`.
#[wasm_bindgen(inline_js = "export function import_notes_editor(url) { return import(url); }")]
extern "C" {
    fn import_notes_editor(url: &str) -> js_sys::Promise;
}

#[wasm_bindgen]
extern "C" {
    /// What the module's `create` returns.
    #[derive(Clone)]
    pub type CodeEditor;
    #[wasm_bindgen(method)]
    pub fn text(this: &CodeEditor) -> String;
    #[wasm_bindgen(method, js_name = selection)]
    fn selection_js(this: &CodeEditor) -> js_sys::Array;
    #[wasm_bindgen(method)]
    pub fn content(this: &CodeEditor) -> web_sys::HtmlElement;
    #[wasm_bindgen(method)]
    pub fn reset(this: &CodeEditor, text: &str);
    #[wasm_bindgen(method, js_name = setText)]
    pub fn set_text(this: &CodeEditor, text: &str, remember: bool);
    #[wasm_bindgen(method)]
    pub fn replace(this: &CodeEditor, text: &str, anchor: u32, head: u32);
    #[wasm_bindgen(method)]
    pub fn insert(this: &CodeEditor, text: &str);
    #[wasm_bindgen(method)]
    pub fn select(this: &CodeEditor, anchor: u32, head: u32, scroll: bool);
    #[wasm_bindgen(method)]
    pub fn focus(this: &CodeEditor);
    #[wasm_bindgen(method, js_name = coords)]
    fn coords_js(this: &CodeEditor, pos: u32) -> JsValue;
    #[wasm_bindgen(method)]
    pub fn undo(this: &CodeEditor) -> bool;
    #[wasm_bindgen(method)]
    pub fn redo(this: &CodeEditor) -> bool;
    #[wasm_bindgen(method, js_name = setReadOnly)]
    pub fn set_read_only(this: &CodeEditor, value: bool);
    #[wasm_bindgen(method)]
    pub fn refresh(this: &CodeEditor);
    #[wasm_bindgen(method)]
    pub fn measure(this: &CodeEditor);
    #[wasm_bindgen(method)]
    fn destroy(this: &CodeEditor);
}

impl CodeEditor {
    /// The main selection as (from, to).
    pub fn selection(&self) -> (u32, u32) {
        let range = self.selection_js();
        let at = |i: u32| range.get(i).as_f64().unwrap_or(0.0) as u32;
        (at(0), at(1))
    }

    /// The viewport point just below a position (for the suggestion list).
    pub fn point_below(&self, pos: u32) -> Option<(f64, f64)> {
        let rect = self.coords_js(pos);
        let read = |key: &str| {
            js_sys::Reflect::get(&rect, &JsValue::from_str(key))
                .ok()
                .and_then(|v| v.as_f64())
        };
        Some((read("left")?, read("bottom")? + 4.0))
    }
}

/// How the editor starts.
pub struct Options<'a> {
    pub text: &'a str,
    pub markdown: bool,
    pub read_only: bool,
    pub placeholder: &'a str,
    /// Attributes of the content element (its accessible name, the suggestion list it controls).
    pub attributes: &'a [(&'a str, String)],
}

/// What the editor calls. Each runs while the editor lives, so they reach signals with `try_`.
pub struct Callbacks {
    /// The new text when the document changed, whether undo and redo can run, and whether the
    /// editor has focus; after every change of the text or the selection.
    pub update: Box<dyn Fn(Option<String>, bool, bool, bool)>,
    /// A key before the editor handles it; true when Rust took it.
    pub keydown: Box<dyn Fn(web_sys::KeyboardEvent) -> bool>,
    /// Files pasted or dropped; true when Rust took them.
    pub files: Box<dyn Fn(Vec<web_sys::File>) -> bool>,
    /// A link followed (Ctrl/Cmd+click, Alt+Enter or Ctrl/Cmd+Enter): destination, wikilink.
    pub link: Box<dyn Fn(String, bool)>,
    pub tag: Box<dyn Fn(String)>,
    pub blur: Box<dyn Fn()>,
    /// The class a link destination shows with (`is-unresolved`, `is-external`, `is-file`).
    pub resolve: Box<dyn Fn(String, bool) -> String>,
}

/// A live editor with the closures it calls. Dropping it destroys the editor, then the closures.
pub struct Editor {
    handle: CodeEditor,
    _closures: Vec<Box<dyn std::any::Any>>,
}

impl Editor {
    pub fn handle(&self) -> CodeEditor {
        self.handle.clone()
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.handle.destroy();
    }
}

thread_local! {
    static MODULE: RefCell<Option<JsValue>> = const { RefCell::new(None) };
}

/// Imports the editor module once.
pub async fn module() -> Result<JsValue, String> {
    if let Some(module) = MODULE.with(|m| m.borrow().clone()) {
        return Ok(module);
    }
    let url = document()
        .document_element()
        .and_then(|root| root.get_attribute("data-notes-editor"))
        .ok_or_else(|| "The editor is not part of this build.".to_owned())?;
    let module = JsFuture::from(import_notes_editor(&url))
        .await
        .map_err(|_| {
            "The editor could not be loaded. Check the connection and open the note again."
                .to_owned()
        })?;
    MODULE.with(|m| *m.borrow_mut() = Some(module.clone()));
    Ok(module)
}

fn set(target: &js_sys::Object, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(target, &JsValue::from_str(key), value);
}

/// Builds an editor in `parent`.
pub fn create(
    module: &JsValue,
    parent: &web_sys::HtmlElement,
    options: Options<'_>,
    callbacks: Callbacks,
) -> Result<Editor, String> {
    let create = js_sys::Reflect::get(module, &JsValue::from_str("create"))
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .ok_or_else(|| "The editor could not be loaded.".to_owned())?;
    let object = js_sys::Object::new();
    set(&object, "text", &JsValue::from_str(options.text));
    set(&object, "markdown", &JsValue::from_bool(options.markdown));
    set(&object, "readOnly", &JsValue::from_bool(options.read_only));
    set(&object, "spellcheck", &JsValue::from_bool(options.markdown));
    set(
        &object,
        "placeholder",
        &JsValue::from_str(options.placeholder),
    );
    let attributes = js_sys::Object::new();
    for (name, value) in options.attributes {
        set(&attributes, name, &JsValue::from_str(value));
    }
    set(&object, "attributes", &attributes);

    let Callbacks {
        update,
        keydown,
        files,
        link,
        tag,
        blur,
        resolve,
    } = callbacks;
    let update = Closure::<dyn Fn(JsValue, bool, bool, bool)>::new(
        move |text: JsValue, undo: bool, redo: bool, focused: bool| {
            update(text.as_string(), undo, redo, focused)
        },
    );
    let keydown = Closure::<dyn Fn(web_sys::KeyboardEvent) -> bool>::new(keydown);
    let files = Closure::<dyn Fn(js_sys::Array) -> bool>::new(move |list: js_sys::Array| {
        files(list.iter().filter_map(|f| f.dyn_into().ok()).collect())
    });
    let link = Closure::<dyn Fn(String, bool)>::new(link);
    let tag = Closure::<dyn Fn(String)>::new(tag);
    let blur = Closure::<dyn Fn()>::new(blur);
    let resolve = Closure::<dyn Fn(String, bool) -> String>::new(resolve);
    set(&object, "onUpdate", update.as_ref());
    set(&object, "onKeydown", keydown.as_ref());
    set(&object, "onFiles", files.as_ref());
    set(&object, "onLink", link.as_ref());
    set(&object, "onTag", tag.as_ref());
    set(&object, "onBlur", blur.as_ref());
    set(&object, "resolve", resolve.as_ref());

    let handle = create
        .call2(&JsValue::NULL, parent, &object)
        .map_err(|_| "The editor could not start.".to_owned())?;
    Ok(Editor {
        handle: handle.unchecked_into(),
        _closures: vec![
            Box::new(update),
            Box::new(keydown),
            Box::new(files),
            Box::new(link),
            Box::new(tag),
            Box::new(blur),
            Box::new(resolve),
        ],
    })
}

/// What following a link in the editor does.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkAction {
    /// A note: target, wikilink, fragment (as the preview's links follow them).
    Note(String, bool, Option<String>),
    /// A web address or a file's content, in a new tab.
    Open(String),
    /// Nothing to follow (another file named by a wikilink, an unknown scheme).
    None,
}

fn web_link(target: &str) -> bool {
    let lower = target.to_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
}

/// Reads a link destination as the preview does: wikilinks name notes (or other files);
/// Markdown links are web addresses, file contents or relative `.md` paths.
pub fn link_action(dest: &str, wiki: bool) -> LinkAction {
    let dest = dest.trim();
    if wiki {
        let (target, fragment) = split_fragment(dest);
        if target.is_empty() && fragment.is_none() {
            return LinkAction::None;
        }
        return if dar_notes::is_note_target(&target) {
            LinkAction::Note(target, true, fragment)
        } else {
            LinkAction::None
        };
    }
    let dest = dest.trim_start_matches('<').trim_end_matches('>');
    if web_link(dest) || dest.starts_with("/api/v1/files/") {
        return LinkAction::Open(dest.to_owned());
    }
    let path = dest.split('#').next().unwrap_or_default().to_lowercase();
    if !dest.contains(':')
        && !dest.starts_with('#')
        && (path.ends_with(".md") || path.ends_with(".markdown"))
    {
        let (path, fragment) = split_fragment(dest);
        return LinkAction::Note(
            dar_notes::percent_decode(&path),
            false,
            fragment.map(|f| dar_notes::percent_decode(&f)),
        );
    }
    LinkAction::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_follow_as_in_the_preview() {
        assert_eq!(
            link_action("Apollo#Launch", true),
            LinkAction::Note("Apollo".into(), true, Some("Launch".into()))
        );
        assert_eq!(
            link_action("Proj/Plan", true),
            LinkAction::Note("Proj/Plan".into(), true, None)
        );
        assert_eq!(link_action("photo.png", true), LinkAction::None);
        assert_eq!(link_action("", true), LinkAction::None);
        assert_eq!(
            link_action("Work%20x/Plan.md#Sub%20part", false),
            LinkAction::Note("Work x/Plan.md".into(), false, Some("Sub part".into()))
        );
        assert_eq!(
            link_action("https://example.com/a", false),
            LinkAction::Open("https://example.com/a".into())
        );
        assert_eq!(
            link_action("<https://example.com>", false),
            LinkAction::Open("https://example.com".into())
        );
        assert_eq!(link_action("javascript:alert(1)", false), LinkAction::None);
        assert_eq!(link_action("other.txt", false), LinkAction::None);
    }
}
