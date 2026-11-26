//! Books reader: EPUB, PDF, and Comic/Manga reading stage with Focus mode, format-aware toolbar,
//! anchored Aa typography popover, Find bar, 3-tab Contents/Bookmarks/Highlights drawer,
//! chapter-tick footer scrubber with clickable page jump, and cross-device conflict resolution.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use js_sys::Reflect;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde_json::{Map, Value, json};
use wasm_bindgen::{JsCast, JsValue};

use crate::api::{self, BrowserSession, Req};
use crate::books::comic::{ComicHandle, ComicView};
use crate::books::epub::{EpubHandle, EpubView, ReaderLook};
use crate::books::highlights::{self, SavedHighlight};
use crate::books::model::{
    API, BookDetail, BookItem, BookManifest, BookMark, BookPosition, BookState, BookTocEntry,
    Conflict, FONT_SIZES, Where, call_promise_method, default_look, device_name, fixed_theme,
    is_fullscreen, line_height_value, percent, pref_str, pref_u32, time_left,
};
use crate::books::pdf::{PdfHandle, PdfOutlineEntry, PdfReady, PdfView, PdfZoomChange};
use crate::books::pdf_text::{PdfFindState, PdfHighlight, PdfSelection};
use crate::dates::edited;
use crate::icons::Icon;
use crate::settings::Segmented;
use crate::shared::context;
use crate::ui::{ActionMenu, MenuAction, Select, document, listen, window};

/// The reader in the middle sheet: bar, format stage, 3-tab drawer, and chapter-tick scrubber.
#[component]
pub fn Reader(
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
    let find_open = RwSignal::new(false);
    let find_text = RwSignal::new(String::new());
    let find_state: RwSignal<PdfFindState> = RwSignal::new(PdfFindState::default());
    let find_input = NodeRef::<html::Input>::new();
    let saved_highlights: RwSignal<Vec<SavedHighlight>> = RwSignal::new(Vec::new());
    let hl_note_id: RwSignal<Option<String>> = RwSignal::new(None);
    let pdf_highlights = Memo::new(move |_| {
        saved_highlights.with(|list| {
            list.iter()
                .map(|h| PdfHighlight {
                    id: h.id.clone(),
                    rects: h.rects.clone(),
                })
                .collect::<Vec<_>>()
        })
    });
    let pdf_scale = RwSignal::new(1.0_f64);
    let focus_mode = RwSignal::new(false);
    let page_jump_open = RwSignal::new(false);
    let page_jump_val = RwSignal::new(String::new());
    let page_jump_input = NodeRef::<html::Input>::new();
    let comic_fit = RwSignal::new("height".to_owned());

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
            let loaded = highlights::load_detailed(&bid, &title).await;
            let _ = hl_note_id.try_set(loaded.note_id);
            let _ = saved_highlights.try_set(loaded.highlights);
        });
    }

    // Focus mode toggles `is-book-focus` on body to collapse the global Dar top bar.
    Effect::new(move |_| {
        let on = focus_mode.get();
        if let Some(body) = document().body() {
            let _ = body.class_list().toggle_with_force("is-book-focus", on);
        }
    });
    on_cleanup(|| {
        if let Some(body) = document().body() {
            let _ = body.class_list().remove_1("is-book-focus");
        }
    });

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
    let aa = NodeRef::<html::Button>::new();
    let aa_panel = NodeRef::<html::Div>::new();

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
    let columns_pref = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksColumns"))
                .unwrap_or("auto")
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
    let comic_layout = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksComicLayout"))
                .unwrap_or("single")
                .to_owned()
        })
    });
    let pdf_tint = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksPdfTint"))
                .unwrap_or("paper")
                .to_owned()
        })
    });

    // Compute ReaderLook from preferences & stage styles.
    Effect::new(move |_| {
        let _has_detail = detail.with(Option::is_some);
        let t = theme.get();
        let f = font.get();
        let sz = font_size.get();
        let lh = line_height_value(&line_height_pref.get());
        let ly = layout.get();
        let cols = columns_pref.get();
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
            columns: cols,
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
                        let _ = on_state.try_run(st);
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
                                let _ = on_state.try_run(updated);
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
            } else if page_jump_open.get_untracked() {
                page_jump_open.set(false);
            } else if focus_mode.get_untracked() {
                focus_mode.set(false);
            } else if is_fullscreen() {
                call_promise_method(&document(), "exitFullscreen");
            } else {
                let _ = on_close.try_run(());
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
        } else if key == "z" && !event.ctrl_key() && !event.meta_key() {
            event.prevent_default();
            focus_mode.update(|f| *f = !*f);
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
            let ch = chapter_label(Some(&w.position));
            let est = book_sv.with_value(|b| time_left(b, w.progress));
            match (ch.is_empty(), est) {
                (false, Some(e)) => format!("{ch} \u{00b7} {e}"),
                (false, None) => ch,
                (true, Some(e)) => e,
                (true, None) => String::new(),
            }
        }
    });

    // Scrubber chapter/section tick positions (fractions in 0.0..1.0).
    let scrubber_ticks = Memo::new(move |_| {
        detail.with(|d| match d.as_ref().map(|d| &d.manifest) {
            Some(BookManifest::Epub { spine, toc, .. }) => {
                let total: f64 = spine.iter().map(|s| s.size).sum::<f64>().max(1.0);
                let mut ticks = Vec::new();
                for entry in toc.iter().filter(|e| e.depth == 0 && e.spine > 0) {
                    let before: f64 = spine
                        .iter()
                        .take(entry.spine as usize)
                        .map(|s| s.size)
                        .sum();
                    let frac = (before / total).clamp(0.02, 0.98);
                    ticks.push((frac, entry.label.clone()));
                }
                ticks.truncate(24);
                ticks
            }
            Some(BookManifest::Pdf) => {
                let Some(info) = pdf_info.get() else {
                    return Vec::new();
                };
                if info.pages <= 1 {
                    return Vec::new();
                }
                let denom = (info.pages - 1) as f64;
                info.outline
                    .iter()
                    .filter(|e| e.depth == 0 && e.page > 0)
                    .map(|e| (((e.page as f64) / denom).clamp(0.02, 0.98), e.label.clone()))
                    .take(24)
                    .collect()
            }
            _ => Vec::new(),
        })
    });

    let set_pref = move |key: &str, val: Value| {
        let mut map = Map::new();
        map.insert(key.into(), val);
        let _ = on_preferences.try_run(map);
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
    let is_pdf_book = book.format == "pdf";

    let open_page_jump = move |_| {
        let cur = current_where
            .with_untracked(|w| w.as_ref().and_then(|w| w.position.page))
            .unwrap_or(0);
        page_jump_val.set((cur + 1).to_string());
        page_jump_open.set(true);
        spawn_local(async move {
            crate::sleep(0).await;
            if let Some(inp) = page_jump_input.try_get_untracked().flatten() {
                let _ = inp.focus();
                inp.select();
            }
        });
    };

    let commit_page_jump = move || {
        let max_p = total_pages.get_untracked();
        if let Ok(p) = page_jump_val.get_untracked().trim().parse::<usize>()
            && p >= 1
            && (max_p == 0 || p <= max_p)
        {
            go_to(BookPosition {
                spine: None,
                page: Some(p - 1),
                fraction: 0.0,
            });
        }
        page_jump_open.set(false);
    };

    view! {
        <main
            node_ref=root
            class=move || format!(
                "pim-main books-reader books-theme-{}{}{}",
                theme.get(),
                if panel.with(Option::is_some) { " has-panel" } else { "" },
                if focus_mode.get() { " is-focus" } else { "" }
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
                                <button class="icon-button" aria-label="Zoom out" title="Zoom out" on:click=move |_| { if let Some(h) = pdf_handle.get_untracked() { h.zoom(PdfZoomChange::Out); } }><Icon name="minus"/></button>
                                <span class="books-zoom-pct" aria-live="polite">{move || format!("{}%", (pdf_scale.get() * 100.0).round() as i64)}</span>
                                <button class="icon-button" aria-label="Zoom in" title="Zoom in" on:click=move |_| { if let Some(h) = pdf_handle.get_untracked() { h.zoom(PdfZoomChange::In); } }><Icon name="plus"/></button>
                                <button class="icon-button" aria-label="Fit width" title="Fit width" on:click=move |_| { if let Some(h) = pdf_handle.get_untracked() { h.zoom(PdfZoomChange::Width); } }><Icon name="fitWidth"/></button>
                                <button class="icon-button" aria-label="Fit page" title="Fit whole page" on:click=move |_| { if let Some(h) = pdf_handle.get_untracked() { h.zoom(PdfZoomChange::Page); } }><Icon name="spread"/></button>
                            </span>
                            <button
                                class=move || if pdf_tint.get() != "paper" { "icon-button is-active" } else { "icon-button" }
                                aria-label="Paper tint"
                                title=move || match pdf_tint.get().as_str() {
                                    "sepia" => "Paper tint: Sepia (click for Night)",
                                    "night" => "Paper tint: Night (click for Paper)",
                                    _ => "Paper tint: Paper (click for Sepia)",
                                }
                                on:click=move |_| {
                                    let next = match pdf_tint.get_untracked().as_str() {
                                        "paper" => "sepia",
                                        "sepia" => "night",
                                        _ => "paper",
                                    };
                                    set_pref("booksPdfTint", json!(next));
                                }
                            >
                                <Icon name="palette"/>
                            </button>
                        })
                    }}
                    {move || {
                        let is_comic_kind = detail.with(|d| matches!(d.as_ref().map(|d| &d.manifest), Some(BookManifest::Comic { .. })));
                        is_comic_kind.then(|| view! {
                            <div class="books-comic-seg" role="group" aria-label="Comic layout">
                                {[("single", "Single"), ("spread", "Spread"), ("strip", "Strip")].into_iter().map(|(val, lbl)| view! {
                                    <button
                                        type="button"
                                        class=move || if comic_layout.get() == val { "is-active" } else { "" }
                                        aria-pressed=move || (comic_layout.get() == val).to_string()
                                        on:click=move |_| set_pref("booksComicLayout", json!(val))
                                    >{lbl}</button>
                                }).collect_view()}
                            </div>
                            <button
                                type="button"
                                class="books-dir-pill"
                                title="Toggle reading direction (RTL / LTR)"
                                on:click=move |_| {
                                    let next = if right_to_left.get_untracked() { "ltr" } else { "rtl" };
                                    set_pref("booksComicDirection", json!(next));
                                }
                            >
                                {move || if right_to_left.get() { "RTL" } else { "LTR" }}
                            </button>
                            <button
                                type="button"
                                class=move || if comic_fit.get() == "width" { "icon-button is-active" } else { "icon-button" }
                                aria-label=move || if comic_fit.get() == "width" { "Fit height" } else { "Fit width" }
                                title=move || if comic_fit.get() == "width" { "Fit height" } else { "Fit width" }
                                on:click=move |_| comic_fit.update(|f| *f = if f == "width" { "height".into() } else { "width".into() })
                            >
                                <Icon name="fitWidth"/>
                            </button>
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
                        title="Contents, bookmarks and highlights"
                        on:click=move |_| panel.update(|p| *p = if p.is_some() { None } else { Some("contents") })
                    >
                        <Icon name="panel"/>
                    </button>
                    <button
                        class=move || if focus_mode.get() { "icon-button books-focus-btn is-active" } else { "icon-button books-focus-btn" }
                        aria-pressed=move || focus_mode.get().to_string()
                        aria-label="Focus mode"
                        title="Focus mode (Z)"
                        on:click=move |_| focus_mode.update(|f| *f = !*f)
                    >
                        <Icon name="eye"/>
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

            <div class="books-reader-body">
                <div node_ref=stage class="books-stage">
                    <span class="books-accent-probe" aria-hidden="true"></span>

                    // Anchored inside the stage so Aa never overlaps the right drawer!
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
                            <Show when=move || layout.get() == "pages">
                                <span class="books-typography-label">"Columns"</span>
                                <Segmented
                                    label="Columns"
                                    value=Signal::derive(move || columns_pref.get())
                                    options=vec![("auto", "Auto"), ("1", "1-page"), ("2", "2-page")]
                                    on_change=Callback::new(move |v: String| set_pref("booksColumns", json!(v)))
                                />
                            </Show>
                            <p class="books-hint">"Saved to your account for every book."</p>
                        </div>
                    </Show>

                    // Subtle desktop hover edge arrows for paged EPUBs and Comics.
                    {move || {
                        let show_edges = detail.with(|d| match d.as_ref().map(|d| &d.manifest) {
                            Some(BookManifest::Epub { .. }) => layout.get() == "pages",
                            Some(BookManifest::Comic { .. }) => comic_layout.get() != "strip",
                            _ => false,
                        });
                        show_edges.then(|| {
                            let rtl = right_to_left.get();
                            view! {
                                <button
                                    type="button"
                                    class="books-page-edge is-prev"
                                    aria-label=if rtl { "Next page" } else { "Previous page" }
                                    on:click=move |_| step(if rtl { 1 } else { -1 })
                                >
                                    <Icon name="back"/>
                                </button>
                                <button
                                    type="button"
                                    class="books-page-edge is-next"
                                    aria-label=if rtl { "Previous page" } else { "Next page" }
                                    on:click=move |_| step(if rtl { -1 } else { 1 })
                                >
                                    <Icon name="chevron"/>
                                </button>
                            }
                        })
                    }}

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
                                        right_to_left=right_to_left
                                        layout=comic_layout
                                        fit=comic_fit
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
                                        highlights=Signal::from(pdf_highlights)
                                        tint=Signal::from(pdf_tint)
                                        on_scale=Callback::new(move |sc: f64| pdf_scale.set(sc))
                                        on_find=Callback::new(move |state: PdfFindState| find_state.set(state))
                                        on_highlight=Callback::new(move |selection: PdfSelection| {
                                            let (bid, title) = (hl_book.clone(), hl_title.clone());
                                            let quote_text = selection.text.clone();
                                            let hl_page = selection.page;
                                            let session = session_sv.get_value();
                                            spawn_local(async move {
                                                match highlights::save(&session, &bid, &title, &selection).await {
                                                    Ok(saved) => {
                                                        let _ = saved_highlights.try_update(|list| {
                                                            list.push(SavedHighlight {
                                                                id: saved.id,
                                                                page: hl_page,
                                                                quote: quote_text,
                                                                rects: saved.rects,
                                                            });
                                                        });
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
                                {is_pdf_book.then(|| view! {
                                    <button
                                        role="tab"
                                        aria-selected=(active_tab == "highlights").to_string()
                                        on:click=move |_| panel.set(Some("highlights"))
                                    >
                                        "Highlights"
                                        {move || {
                                            let n = saved_highlights.with(Vec::len);
                                            (n > 0).then(|| view! { <small>{n}</small> })
                                        }}
                                    </button>
                                })}
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
                                    } else if active_tab == "highlights" {
                                        let list = saved_highlights.get();
                                        if list.is_empty() {
                                            return view! {
                                                <p class="books-hint">"Select text on any page and choose Highlight. Highlights are stored in Notes \u{203a} Highlights."</p>
                                            }.into_any();
                                        }
                                        let note_href = hl_note_id.get().map(|nid| format!("/notes?note={nid}"));
                                        view! {
                                            <div class="books-panel-hl">
                                                {note_href.map(|href| view! {
                                                    <a class="books-panel-notes-link" href=href>
                                                        <Icon name="note"/>"Open highlights in Notes"
                                                    </a>
                                                })}
                                                {list.into_iter().map(|hl| {
                                                    let page = hl.page;
                                                    let quote = if hl.quote.is_empty() {
                                                        format!("Highlighted passage on page {}", page + 1)
                                                    } else {
                                                        hl.quote
                                                    };
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class="books-hl-drawer-item"
                                                            on:click=move |_| {
                                                                go_to(BookPosition { spine: None, page: Some(page), fraction: 0.0 });
                                                            }
                                                        >
                                                            <blockquote>{quote}</blockquote>
                                                            <small>{format!("Page {}", page + 1)}</small>
                                                        </button>
                                                    }
                                                }).collect_view()}
                                            </div>
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
                                            // Fix dangling separator dot when page is None (EPUBs).
                                            let meta = match m.position.page {
                                                Some(p) => format!("Page {} \u{00b7} {}", p + 1, edited(m.created_at)),
                                                None => edited(m.created_at),
                                            };
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
                {move || {
                    let has_page = current_where.with(|w| w.as_ref().and_then(|w| w.position.page).is_some());
                    if has_page && page_jump_open.get() {
                        let pages = total_pages.get();
                        let max_attr = if pages > 0 {
                            pages.to_string()
                        } else {
                            "99999".into()
                        };
                        view! {
                            <form
                                class="books-page-jump"
                                on:submit=move |ev| {
                                    ev.prevent_default();
                                    commit_page_jump();
                                }
                            >
                                <span>"Page"</span>
                                <input
                                    node_ref=page_jump_input
                                    type="number"
                                    min="1"
                                    max=max_attr
                                    aria-label="Go to page"
                                    prop:value=move || page_jump_val.get()
                                    on:input=move |ev| page_jump_val.set(event_target_value(&ev))
                                    on:blur=move |_| commit_page_jump()
                                    on:keydown=move |ev| {
                                        if ev.key() == "Escape" {
                                            ev.prevent_default();
                                            ev.stop_propagation();
                                            page_jump_open.set(false);
                                        }
                                    }
                                />
                                {(pages > 0).then(|| view! { <span>{format!("of {pages}")}</span> })}
                            </form>
                        }.into_any()
                    } else if has_page {
                        view! {
                            <button
                                type="button"
                                class="books-foot-place is-jumpable"
                                title="Click to jump to a page"
                                on:click=open_page_jump
                            >
                                {move || footer_text.get()}
                            </button>
                        }.into_any()
                    } else {
                        view! {
                            <span class="books-foot-place">{move || footer_text.get()}</span>
                        }.into_any()
                    }
                }}
                <div class="books-scrubber">
                    <div class="books-scrubber-ticks" aria-hidden="true">
                        {move || {
                            scrubber_ticks.get().into_iter().map(|(frac, label)| {
                                let tick_ref = NodeRef::<html::Span>::new();
                                Effect::new(move |_| {
                                    if let Some(el) = tick_ref.get() {
                                        let _ = web_sys::HtmlElement::style(&el)
                                            .set_property("--tick-left", &format!("{:.2}%", frac * 100.0));
                                    }
                                });
                                view! { <span node_ref=tick_ref class="books-scrubber-tick" title=label></span> }
                            }).collect_view()
                        }}
                    </div>
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
                </div>
                <span class="books-foot-pct">{move || percent(progress_value.get())}</span>
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
