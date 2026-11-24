//! Canvases in Notes: Obsidian's JSON Canvas files (`*.canvas`) in the Notes folder, edited in
//! the right sheet. Cards are DOM elements placed by CSS custom properties, edges SVG curves;
//! the file is read and written through the Files APIs with revision checks, autosave,
//! recovery drafts and explicit conflicts, as in the note editor. Fields Dar does not know are
//! written back unchanged (`model.rs`).

mod board;
mod cards;
mod dialogs;
mod gestures;
pub mod model;

pub use model::{is_canvas, stem, unique_name};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::note_links::NoteIndex;
use super::notes::{fetch_detail, read_file_content};
use board::{Board, CanvasDialog};
use cards::{CanvasCard, CanvasEdge, ConnectPreview, Marquee};
use dialogs::CanvasDialogs;
use gestures::Gestures;
use model::{End, Rect};
use crate::api::BrowserSession;
use crate::dates::edited;
use crate::files::FileEntry;
use crate::files::editor::{TextFile, write_text_file};
use crate::icons::Icon;
use crate::live::Live;
use crate::markdown::NoteLinks;
use crate::ui::{ActionMenu, Listener, MenuAction, listen, unique_id, use_body_class, window};

/// The media type new canvases are written with (Obsidian's files are JSON).
pub const MEDIA_TYPE: &str = "application/json";
const MAX_BYTES: f64 = 1024.0 * 1024.0;

/// The text of a new, empty canvas.
pub fn empty_canvas() -> String {
    model::serialize(&model::Canvas::default())
}

fn session_storage() -> Option<web_sys::Storage> {
    window().session_storage().ok().flatten()
}

/// A wheel listener that may prevent scrolling (`passive: false`), removed when dropped.
struct WheelListener {
    target: web_sys::EventTarget,
    callback: Closure<dyn FnMut(web_sys::Event)>,
}

impl WheelListener {
    fn new(target: &web_sys::EventTarget, f: impl FnMut(web_sys::Event) + 'static) -> Self {
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(f);
        let options = web_sys::AddEventListenerOptions::new();
        options.set_passive(false);
        let _ = target.add_event_listener_with_callback_and_add_event_listener_options("wheel", callback.as_ref().unchecked_ref(), &options);
        Self { target: target.clone(), callback }
    }
}

impl Drop for WheelListener {
    fn drop(&mut self) {
        let _ = self.target.remove_event_listener_with_callback("wheel", self.callback.as_ref().unchecked_ref());
    }
}

const COLORS: [(&str, &str); 6] = model::PRESETS;

#[component]
pub fn NoteCanvas(
    entry: Signal<FileEntry>,
    session: BrowserSession,
    #[prop(into)] context: Signal<Option<String>>,
    on_close: Callback<()>,
    on_saved: Callback<(u32, String)>,
    actions: Callback<(), Vec<MenuAction>>,
    links: NoteLinks,
    index: RwSignal<Arc<NoteIndex>>,
    /// Opens a note (a note card's double click).
    on_open: Callback<String>,
) -> impl IntoView {
    use_body_class(&["has-note-canvas"]);
    let live = expect_context::<Live>();
    let initial = entry.get_untracked();
    let status = RwSignal::new("Loading\u{2026}");
    let error = RwSignal::new(String::new());
    let ready = RwSignal::new(false);
    let read_only = RwSignal::new(true);
    // The file is not JSON Canvas: shown as an error, never overwritten.
    let invalid = RwSignal::new(false);
    let discard = RwSignal::new(false);
    let recovery: RwSignal<Option<String>> = RwSignal::new(None);
    let conflict: RwSignal<Option<(String, FileEntry)>> = RwSignal::new(None);
    let modified = RwSignal::new(initial.modified_at);
    let revision = RwSignal::new(initial.revision);
    let saving = RwSignal::new(false);
    // The canvas as last saved, in the form Dar writes it.
    let saved = RwSignal::new(String::new());
    let locked = Signal::from(Memo::new(move |_| {
        read_only.get() || !ready.get() || invalid.get() || recovery.with(Option::is_some) || conflict.with(Option::is_some)
    }));
    let board = Board::new(locked, index, links, on_open);
    let gestures = Gestures::new();
    let serialized = Memo::new(move |_| board.doc.with(model::serialize));
    let dirty = Memo::new(move |_| ready.get() && !invalid.get() && serialized.with(|s| saved.with(|v| s != v)));
    let key = StoredValue::new(format!("dar.file-draft.{}.{}", session.user.id, initial.id));
    let session = StoredValue::new(session);
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }

    // A rename or move through the Notes panes raises the revision.
    Effect::new(move |_| {
        let rev = entry.with(|e| e.revision);
        if ready.get_untracked() && !saving.get_untracked() && rev > revision.get_untracked() {
            revision.set(rev);
        }
    });

    // Initial load.
    {
        let alive = alive.clone();
        let id = initial.id.clone();
        spawn_local(async move {
            let outcome: Result<(super::notes::NoteDetail, String), String> = async {
                let conn = live.connection.get_untracked().ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
                let detail = fetch_detail(&conn, &id).await?;
                if detail.entry.kind != "file" || detail.entry.trashed || detail.entry.size > MAX_BYTES {
                    return Err("This canvas cannot be opened here.".into());
                }
                let content = read_file_content(&id).await?;
                Ok((detail, content))
            }
            .await;
            if !alive.load(Ordering::Relaxed) {
                return;
            }
            match outcome {
                Ok((detail, content)) => {
                    revision.set(detail.entry.revision);
                    modified.set(detail.entry.modified_at);
                    let read = detail.role == "read";
                    read_only.set(read);
                    match model::parse(&content) {
                        Ok(canvas) => {
                            let written = model::serialize(&canvas);
                            board.load(canvas);
                            saved.set(written.clone());
                            ready.set(true);
                            status.set(if read { "Read only" } else { "Saved" });
                            if let Some(draft) = session_storage()
                                .and_then(|s| s.get_item(&key.get_value()).ok().flatten())
                                .filter(|d| *d != written && model::parse(d).is_ok())
                            {
                                recovery.set(Some(draft));
                            }
                        }
                        Err(problem) => {
                            invalid.set(true);
                            ready.set(true);
                            status.set("Read only");
                            error.set(problem);
                        }
                    }
                }
                Err(e) => error.set(e),
            }
        });
    }

    // Recovery draft in sessionStorage, and beforeunload protection.
    Effect::new(move |_| {
        let (text, unsaved) = (serialized.get(), dirty.get());
        if !ready.get() || read_only.get() || invalid.get() || recovery.with(Option::is_some) {
            return;
        }
        if let Some(storage) = session_storage() {
            let result = if unsaved { storage.set_item(&key.get_value(), &text) } else { storage.remove_item(&key.get_value()) };
            if result.is_err() {
                error.set("Recovery storage is unavailable. Keep this canvas open until it is saved.".into());
            }
        }
    });
    listen(&window(), "beforeunload", false, move |event| {
        if dirty.get_untracked() {
            event.prevent_default();
        }
    });

    let alive_compare = alive.clone();
    let compare = move || {
        let alive = alive_compare.clone();
        spawn_local(async move {
            let id = entry.with_untracked(|e| e.id.clone());
            let outcome: Result<(String, FileEntry), String> = async {
                let conn = live.connection.get_untracked().ok_or_else(|| "Notes are reconnecting. Try again in a moment.".to_owned())?;
                let detail = fetch_detail(&conn, &id).await?;
                let content = read_file_content(&id).await.map_err(|_| "Saved version unavailable.".to_owned())?;
                Ok((content, detail.entry))
            }
            .await;
            if !alive.load(Ordering::Relaxed) {
                return;
            }
            match outcome {
                Ok(value) => conflict.set(Some(value)),
                Err(e) => error.set(e),
            }
        });
    };

    let alive_save = alive.clone();
    let compare_for_save = compare.clone();
    let save = move || {
        if !ready.get_untracked()
            || read_only.get_untracked()
            || invalid.get_untracked()
            || saving.get_untracked()
            || recovery.with_untracked(Option::is_some)
            || conflict.with_untracked(Option::is_some)
            || !dirty.get_untracked()
        {
            return;
        }
        let content = serialized.get_untracked();
        let expected = revision.get_untracked();
        saving.set(true);
        status.set("Saving\u{2026}");
        error.set(String::new());
        let alive = alive_save.clone();
        let compare_now = compare_for_save.clone();
        spawn_local(async move {
            let (current, sess) = (entry.get_untracked(), session.get_value());
            let media_type = if current.media_type.contains('/') && current.media_type != "application/octet-stream" { current.media_type.clone() } else { MEDIA_TYPE.to_owned() };
            let file = TextFile {
                name: &current.name,
                parent_id: &current.parent_id,
                content: &content,
                media_type: &media_type,
                file_id: Some(&current.id),
                expected_revision: Some(expected),
            };
            let result = write_text_file(&sess, file).await;
            if alive.load(Ordering::Relaxed) {
                match result {
                    Ok(_) => {
                        revision.set(expected + 1);
                        saved.set(content.clone());
                        modified.set(js_sys::Date::now());
                        status.set("Saved");
                        on_saved.try_run((expected + 1, content));
                    }
                    Err(e) => {
                        error.set(e.message.clone());
                        status.set("Save failed");
                        if [409, 412].contains(&e.status) {
                            compare_now();
                        }
                    }
                }
                saving.set(false);
            }
        });
    };

    // Autosave 900 ms after the last change.
    let typed = RwSignal::new(0u32);
    let save_for_auto = save.clone();
    Effect::new(move |_| {
        serialized.track();
        saved.track();
        if !dirty.get() || !error.with(String::is_empty) || !ready.get() || recovery.with(Option::is_some) || conflict.with(Option::is_some) {
            return;
        }
        let ticket = typed.get_untracked() + 1;
        typed.set(ticket);
        let save_now = save_for_auto.clone();
        spawn_local(async move {
            crate::sleep(900).await;
            if typed.try_get_untracked() == Some(ticket) {
                save_now();
            }
        });
    });

    // Fit the cards once the canvas and the stage's size are known.
    let fitted = StoredValue::new(false);
    Effect::new(move |_| {
        let (w, h) = board.size.get();
        if ready.get() && w > 0.0 && h > 0.0 && !fitted.get_value() {
            fitted.set_value(true);
            board.fit();
        }
    });

    // The view as custom properties on the stage (the world layer and the dot grid use them).
    let stage = board.stage;
    Effect::new(move |_| {
        let view = board.view.get();
        let Some(element) = stage.get() else { return };
        let style = element.style();
        let _ = style.set_property("--pan-x", &format!("{}px", view.x));
        let _ = style.set_property("--pan-y", &format!("{}px", view.y));
        let _ = style.set_property("--zoom", &view.zoom.to_string());
    });

    // Pointer, wheel and keys on the stage, and its size; removed with the editor.
    Effect::new(move |_| {
        let Some(div) = stage.get() else { return };
        let element: web_sys::Element = div.clone().into();
        let target: web_sys::EventTarget = div.into();
        let measured = element.clone();
        let observer_callback = Closure::<dyn FnMut()>::new(move || {
            let rect = measured.get_bounding_client_rect();
            let _ = board.size.try_set((rect.width(), rect.height()));
        });
        let observer = web_sys::ResizeObserver::new(observer_callback.as_ref().unchecked_ref()).ok();
        if let Some(observer) = &observer {
            observer.observe(&element);
        }
        let rect = element.get_bounding_client_rect();
        board.size.set((rect.width(), rect.height()));

        let on = |name: &'static str, f: fn(Gestures, Board, &web_sys::Element, web_sys::Event)| {
            let element = element.clone();
            Listener::new(&target, name, false, move |event| f(gestures, board, &element, event))
        };
        let down = on("pointerdown", |g, b, el, event| {
            if let Ok(e) = event.dyn_into::<web_sys::PointerEvent>() {
                g.down(b, el, &e);
            }
        });
        let moved = on("pointermove", |g, b, el, event| {
            if let Ok(e) = event.dyn_into::<web_sys::PointerEvent>() {
                g.moved(b, el, &e);
            }
        });
        let up = on("pointerup", |g, b, el, event| {
            if let Ok(e) = event.dyn_into::<web_sys::PointerEvent>() {
                g.up(b, el, &e, false);
            }
        });
        let cancel = on("pointercancel", |g, b, el, event| {
            if let Ok(e) = event.dyn_into::<web_sys::PointerEvent>() {
                g.up(b, el, &e, true);
            }
        });
        let double = on("dblclick", |g, b, el, event| {
            if let Ok(e) = event.dyn_into::<web_sys::MouseEvent>() {
                g.double_click(b, el, &e);
            }
        });
        let keys = on("keydown", |g, b, el, event| {
            if let Ok(e) = event.dyn_into::<web_sys::KeyboardEvent>() {
                g.key_down(b, el, &e);
            }
        });
        let keys_up = on("keyup", |g, _, el, event| {
            if let Ok(e) = event.dyn_into::<web_sys::KeyboardEvent>() {
                g.key_up(el, &e);
            }
        });
        let click = Listener::new(&target, "click", true, move |event| gestures.click(&event));
        let wheel = {
            let element = element.clone();
            WheelListener::new(&target, move |event| {
                if let Ok(e) = event.dyn_into::<web_sys::WheelEvent>() {
                    gestures.wheel(board, &element, &e);
                }
            })
        };
        let blur = Listener::new(&window(), "blur", false, move |_| gestures.space.set_value(false));
        let kept = send_wrapper::SendWrapper::new((observer, observer_callback, (down, moved, up, cancel, double, keys, keys_up, click), wheel, blur));
        on_cleanup(move || {
            let (observer, callback, listeners, wheel, blur) = kept.take();
            if let Some(observer) = observer {
                observer.disconnect();
            }
            drop((listeners, wheel, blur, callback));
        });
    });

    let phase = move || {
        if read_only.get() || invalid.get() {
            "read"
        } else if matches!(status.get(), "Saving\u{2026}" | "Loading\u{2026}") {
            "saving"
        } else if dirty.get() {
            if status.get() == "Save failed" { "error" } else { "dirty" }
        } else {
            "saved"
        }
    };
    let label = move || {
        let s = status.get();
        if matches!(s, "Loading\u{2026}" | "Saving\u{2026}") || read_only.get() || invalid.get() {
            s
        } else if dirty.get() {
            if s == "Save failed" { "Not saved" } else { "Unsaved changes" }
        } else {
            "All changes saved"
        }
    };

    let close = move || {
        if dirty.get_untracked() || saving.get_untracked() {
            discard.set(true);
        } else {
            on_close.try_run(());
        }
    };

    let add_group = move || {
        let selected = board.selected.get_untracked();
        let area = board.doc.with_untracked(|c| model::bounds(c.nodes.iter().filter(|n| selected.contains(&n.id))));
        let made = match area {
            Some(r) => {
                let r = Rect::new(r.x - 40.0, r.y - 70.0, r.width + 80.0, r.height + 110.0);
                board.add_card("group", "", r.center(), |n| {
                    n.x = r.x;
                    n.y = r.y;
                    n.width = r.width;
                    n.height = r.height;
                })
            }
            None => board.add_card("group", "", board.center(), |_| {}),
        };
        if let Some(id) = made {
            board.begin_edit(&id);
        }
    };

    let color_actions = move |_: ()| {
        let usable = board.has_selection() && !locked.get_untracked();
        let current = board.doc.with_untracked(|c| match board.selected_edge.get_untracked() {
            Some(edge) => c.edges.iter().find(|e| e.id == edge).and_then(|e| e.color.clone()),
            None => board.selected.with_untracked(|s| s.first().and_then(|id| c.node(id)).and_then(|n| n.color.clone())),
        });
        let mut list = vec![MenuAction::plain("No colour").current(current.is_none()).disabled(!usable).run(move || board.set_color(None))];
        for (value, name) in COLORS {
            list.push(MenuAction::plain(name).current(current.as_deref() == Some(value)).disabled(!usable).run(move || board.set_color(Some(value.to_owned()))));
        }
        list
    };
    let edge_actions = move |_: ()| {
        let usable = board.selected_edge.with_untracked(Option::is_some) && !locked.get_untracked();
        let ends = board.doc.with_untracked(|c| board.selected_edge.with_untracked(|id| id.as_ref().and_then(|id| c.edges.iter().find(|e| &e.id == id)).map(|e| (e.from_end, e.to_end))));
        vec![
            MenuAction::new("Edit label", "pencil").disabled(!usable).run(move || {
                if let Some(id) = board.selected_edge.get_untracked() {
                    board.begin_edge_edit(&id);
                }
            }),
            MenuAction::plain("Arrow at the end").separator().current(ends == Some((End::None, End::Arrow))).disabled(!usable).run(move || board.set_ends(End::None, End::Arrow)),
            MenuAction::plain("Arrows at both ends").current(ends == Some((End::Arrow, End::Arrow))).disabled(!usable).run(move || board.set_ends(End::Arrow, End::Arrow)),
            MenuAction::plain("No arrows").current(ends == Some((End::None, End::None))).disabled(!usable).run(move || board.set_ends(End::None, End::None)),
        ]
    };

    let node_ids = Memo::new(move |_| {
        board.doc.with(|c| {
            let mut seen: Vec<String> = Vec::with_capacity(c.nodes.len());
            for node in &c.nodes {
                if !seen.contains(&node.id) {
                    seen.push(node.id.clone());
                }
            }
            seen
        })
    });
    let edge_ids = Memo::new(move |_| {
        board.doc.with(|c| {
            let mut seen: Vec<String> = Vec::with_capacity(c.edges.len());
            for edge in &c.edges {
                if !seen.contains(&edge.id) {
                    seen.push(edge.id.clone());
                }
            }
            seen
        })
    });
    let zoom_label = move || format!("{}%", (board.view.get().zoom * 100.0).round());
    let selection_count = move || board.selected.with(Vec::len);
    let has_selection = move || selection_count() > 0 || board.selected_edge.with(Option::is_some);
    let help_id = unique_id();

    let save_for_key = save.clone();
    let save_for_button = save.clone();
    let save_for_retry = save.clone();

    view! {
        <section class="note-canvas-editor" aria-label=move || format!("Editor for {}", entry.with(|e| e.name.clone()))
            on:keydown=move |e: web_sys::KeyboardEvent| {
                if (e.ctrl_key() || e.meta_key()) && e.key().to_lowercase() == "s" {
                    e.prevent_default();
                    save_for_key();
                } else if e.key() == "Escape" && !e.default_prevented() {
                    e.prevent_default();
                    close();
                }
            }>
            <div class="workspace-editor-top">
                <button class="icon-button" aria-label="Close editor" title="Close editor" on:click=move |_| close()><Icon name="close"/></button>
                <span class="note-notebook">{move || context.get().map(|c| view! { <Icon name="folder"/>{c} })}</span>
                <span class=move || format!("note-status note-status-{}", phase()) role="status">
                    {move || (phase() == "saved").then(|| view! { <Icon name="check"/> })}
                    {label}
                </span>
                {move || (dirty.get() && !read_only.get()).then(|| {
                    let save_now = save_for_button.clone();
                    view! {
                        <button class="text-button" disabled=move || status.get() == "Saving\u{2026}" || conflict.with(Option::is_some) || recovery.with(Option::is_some) on:click=move |_| save_now()>"Save now"</button>
                    }
                })}
                <ActionMenu label="File options" actions=move |_| {
                    let mut list = actions.run(());
                    let separate = !list.is_empty();
                    let id = entry.with_untracked(|e| e.id.clone());
                    let mut download = MenuAction::new("Download", "download").href(format!("/api/v1/files/{id}/content")).download();
                    if separate {
                        download = download.separator();
                    }
                    list.push(download);
                    list
                }><Icon name="more"/></ActionMenu>
            </div>
            {move || discard.get().then(|| view! {
                <div class="draft-banner" role="alert">
                    <p>{if saving.get() { "A save is in progress. Keep the canvas open until it completes." } else { "Unsaved changes are retained as a recovery draft in this tab." }}</p>
                    <button class="text-button" on:click=move |_| discard.set(false)>"Keep editing"</button>
                    <button class="text-button danger" disabled=move || saving.get() on:click=move |_| { on_close.try_run(()); }>"Close with recovery draft"</button>
                </div>
            })}
            {move || (!error.get().is_empty()).then(|| {
                let retry = save_for_retry.clone();
                let review = compare.clone();
                view! {
                    <div class="error-banner" role="alert">
                        <span>{error.get()}</span>
                        {(!invalid.get()).then(|| view! {
                            <button class="text-button" on:click=move |_| retry()>"Retry save"</button>
                            <button class="text-button" on:click=move |_| review()>"Review saved version"</button>
                        })}
                    </div>
                }
            })}
            {move || recovery.get().map(|draft| view! {
                <div class="draft-banner" role="alert">
                    <p>"A recovery draft of this canvas is available."</p>
                    <button class="text-button" on:click=move |_| {
                        recovery.set(None);
                        if let Ok(canvas) = model::parse(&draft) {
                            board.change(move |doc| *doc = canvas);
                        }
                    }>"Restore draft"</button>
                    <button class="text-button" on:click=move |_| {
                        recovery.set(None);
                        if let Some(storage) = session_storage() {
                            let _ = storage.remove_item(&key.get_value());
                        }
                    }>"Discard draft"</button>
                </div>
            })}
            {move || conflict.get().map(|(content, latest)| {
                let (theirs, latest_theirs) = (content.clone(), latest.clone());
                view! {
                    <div class="draft-banner" role="alert">
                        <p>"The saved canvas changed. Your changes have been kept."</p>
                        <details><summary>"View saved version"</summary><pre>{content.clone()}</pre></details>
                        <button class="text-button" on:click=move |_| {
                            match model::parse(&theirs) {
                                Ok(canvas) => {
                                    revision.set(latest_theirs.revision);
                                    modified.set(latest_theirs.modified_at);
                                    saved.set(model::serialize(&canvas));
                                    conflict.set(None);
                                    board.load(canvas);
                                    error.set(String::new());
                                    status.set("Saved");
                                }
                                Err(problem) => error.set(problem),
                            }
                        }>"Use saved version"</button>
                        <button class="text-button" on:click=move |_| {
                            revision.set(latest.revision);
                            conflict.set(None);
                            error.set(String::new());
                            status.set("Unsaved");
                        }>"Keep my version"</button>
                    </div>
                }
            })}
            <div class="note-canvas-heading">
                <h1 class="note-canvas-title"><Icon name="grid"/><span>{move || stem(&entry.with(|e| e.name.clone())).to_owned()}</span></h1>
                <span class="note-meta">"Edited "{move || edited(modified.get())}</span>
            </div>
            <div class="canvas-toolbar" role="toolbar" aria-label="Canvas tools">
                <button type="button" class="icon-button" aria-label="Add text card" title="Add text card" disabled=move || locked.get()
                    on:click=move |_| {
                        if let Some(id) = board.add_card("text", "", board.center(), |_| {}) {
                            board.begin_edit(&id);
                        }
                    }><Icon name="note"/></button>
                <button type="button" class="icon-button" aria-label="Add note or file" title="Add note or file" disabled=move || locked.get()
                    on:click=move |_| board.dialog.set(Some(CanvasDialog::Pick))><Icon name="file"/></button>
                <button type="button" class="icon-button" aria-label="Add link card" title="Add link card" disabled=move || locked.get()
                    on:click=move |_| board.dialog.set(Some(CanvasDialog::Link(None)))><Icon name="link"/></button>
                <button type="button" class="icon-button" aria-label="Add group" title="Add group (around the selection)" disabled=move || locked.get()
                    on:click=move |_| add_group()><Icon name="stack"/></button>
                <span class="canvas-toolbar-gap" aria-hidden="true"></span>
                <ActionMenu label="Colour" class="canvas-tool-menu" actions=color_actions><Icon name="palette"/></ActionMenu>
                <ActionMenu label="Connection options" class="canvas-tool-menu" actions=edge_actions><Icon name="branch"/></ActionMenu>
                <button type="button" class="icon-button" aria-label="Connect selected cards" title="Connect the two selected cards" disabled=move || locked.get() || selection_count() != 2
                    on:click=move |_| { board.connect_selected(); }><Icon name="share"/></button>
                <button type="button" class="icon-button" aria-label="Delete selection" title="Delete selection" disabled=move || locked.get() || !has_selection()
                    on:click=move |_| { board.delete_selection(); }><Icon name="trash"/></button>
                <span class="canvas-toolbar-gap" aria-hidden="true"></span>
                <button type="button" class="icon-button" aria-label="Undo" title="Undo" disabled=move || locked.get() || !board.can_undo.get()
                    on:click=move |_| board.undo()><Icon name="undo"/></button>
                <button type="button" class="icon-button" aria-label="Redo" title="Redo" disabled=move || locked.get() || !board.can_redo.get()
                    on:click=move |_| board.redo()><Icon name="redo"/></button>
                <span class="canvas-toolbar-gap" aria-hidden="true"></span>
                <button type="button" class="icon-button" aria-label="Zoom out" title="Zoom out" on:click=move |_| board.zoom_by(0.8)><Icon name="minus"/></button>
                <span class="canvas-zoom">{zoom_label}</span>
                <button type="button" class="icon-button" aria-label="Zoom in" title="Zoom in" on:click=move |_| board.zoom_by(1.25)><Icon name="plus"/></button>
                <button type="button" class="icon-button" aria-label="Zoom to fit" title="Zoom to fit" on:click=move |_| board.fit()><Icon name="expand"/></button>
            </div>
            <div node_ref=stage class="note-canvas-stage" tabindex="0" role="group" aria-roledescription="canvas" aria-label="Canvas" aria-describedby=help_id.clone()>
                <div class="canvas-world">
                    <For each=move || edge_ids.get() key=|id| id.clone() let:id>
                        <CanvasEdge board=board id=id/>
                    </For>
                    <For each=move || node_ids.get() key=|id| id.clone() let:id>
                        <CanvasCard board=board id=id/>
                    </For>
                    <ConnectPreview board=board/>
                    <Marquee board=board/>
                </div>
                {move || (!ready.get() && error.with(String::is_empty)).then(|| view! { <p class="canvas-hint" role="status">"Loading canvas\u{2026}"</p> })}
                {move || (ready.get() && !invalid.get() && node_ids.with(Vec::is_empty)).then(|| view! {
                    <p class="canvas-hint">{if read_only.get() { "This canvas is empty." } else { "Double-click to add a text card, or use the tools above." }}</p>
                })}
            </div>
            <p id=help_id class="sr-only">
                "Drag the background, use the wheel or the arrow keys to pan; Ctrl or Command with the wheel zooms. Double-click to add a text card. "
                "Tab moves between cards; arrow keys move the selected cards (Shift for larger steps), Enter edits, Delete removes and Escape clears the selection. "
                "Drag from a card's side to connect it; Shift+drag selects an area."
            </p>
            <CardOutline board=board/>
            <CanvasDialogs board=board/>
        </section>
    }
}

/// The cards and connections as lists: the keyboard and screen reader alternative to the canvas.
#[component]
fn CardOutline(board: Board) -> impl IntoView {
    let cards = Memo::new(move |_| board.doc.with(|c| c.nodes.iter().map(|n| (n.id.clone(), format!("{}: {}", model::kind_label(&n.kind), model::summary(n)))).collect::<Vec<_>>()));
    let connections = Memo::new(move |_| {
        board.doc.with(|c| {
            let name = |id: &str| c.node(id).map(model::summary).unwrap_or_else(|| "Missing card".into());
            c.edges
                .iter()
                .map(|e| {
                    let label = e.label.as_deref().filter(|l| !l.is_empty()).map(|l| format!(" ({l})")).unwrap_or_default();
                    (e.id.clone(), format!("{} \u{2192} {}{label}", name(&e.from_node), name(&e.to_node)))
                })
                .collect::<Vec<_>>()
        })
    });
    view! {
        <details class="canvas-outline">
            <summary>{move || format!("Cards ({}) and connections ({})", cards.with(Vec::len), connections.with(Vec::len))}</summary>
            <ul class="canvas-outline-list" role="list" aria-label="Canvas cards">
                {move || cards.get().into_iter().map(|(id, text)| {
                    let (pressed, chosen) = (id.clone(), id);
                    view! {
                        <li>
                            <button type="button" class="canvas-outline-item" aria-pressed=move || board.selected.with(|s| s.contains(&pressed)).to_string()
                                on:click=move |_| {
                                    board.select_only(&chosen);
                                    board.reveal(&chosen);
                                    board.focus_card(&chosen);
                                }>{text}</button>
                        </li>
                    }
                }).collect_view()}
            </ul>
            {move || (!connections.with(Vec::is_empty)).then(|| view! {
                <ul class="canvas-outline-list" role="list" aria-label="Canvas connections">
                    {connections.get().into_iter().map(|(id, text)| {
                        let (pressed, chosen) = (id.clone(), id);
                        view! {
                            <li>
                                <button type="button" class="canvas-outline-item" aria-pressed=move || board.selected_edge.with(|s| s.as_deref() == Some(pressed.as_str())).to_string()
                                    on:click=move |_| board.select_edge(&chosen)>{text}</button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            })}
        </details>
    }
}
