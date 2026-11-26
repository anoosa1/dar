//! Books workspace coordinator: wires library loading, sidebar navigation, Home shelves,
//! Catalog (Grid / List / Series), Highlights browser, Details inspector, Reader, Settings,
//! drag-and-drop uploads, bulk actions, and toasts inside Dar's shared frame.

use std::collections::HashSet;
use std::sync::Arc;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde_json::{Map, Value, json};
use wasm_bindgen::{JsCast, JsValue};

use crate::api::{self, BrowserSession, Req};
use crate::books::catalog::{BooksCatalogGrid, BooksFilterBar, BooksHighlightsView};
use crate::books::details::BookDetails;
use crate::books::highlights::{self, HighlightNoteSummary};
use crate::books::home::BooksHome;
use crate::books::model::{
    API, BookItem, BookLibrary, BookState, BookTransfer, CatalogFilter, SORTS, UploadReservation,
    VIEWS, View, book_format, dedupe_filename, filter_books, media_type, percent, pref_str,
    read_view, status_label, upload_blob_xhr,
};
use crate::books::reader::Reader;
use crate::books::settings::BooksSettings;
use crate::books::sidebar::BooksSidebar;
use crate::icons::Icon;
use crate::live::{Live, reducer, replied};
use crate::module_bindings::trash_entry_reducer::trash_entry as _;
use crate::settings::Notice;
use crate::shared::{PREFERENCES, Preferences, context, save_preferences};
use crate::shell::WorkspaceHeader;
use crate::ui::{
    ActionMenu, Dialog, MenuAction, active_element, document, listen, unique_id, window,
};

#[component]
pub fn BooksWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let (start_view, start_book, start_read, start_page) = read_view();
    // A page from the address opens the first reader at that page, once.
    let deep_page: StoredValue<Option<(String, usize)>> =
        StoredValue::new(start_book.clone().zip(start_page));
    let library: RwSignal<Option<BookLibrary>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let view = RwSignal::new(start_view);
    let filter = RwSignal::new(CatalogFilter::All);
    let selected: RwSignal<Option<String>> = RwSignal::new(start_book.clone());
    let reading: RwSignal<Option<String>> =
        RwSignal::new(if start_read { start_book } else { None });
    let settings = RwSignal::new(false);
    let notice: RwSignal<Option<Notice>> = RwSignal::new(None);
    let reload = RwSignal::new(0u32);
    let transfers: RwSignal<Vec<BookTransfer>> = RwSignal::new(Vec::new());
    let confirm: RwSignal<Option<Vec<BookItem>>> = RwSignal::new(None);
    let selecting = RwSignal::new(false);
    let checked: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let dragging = RwSignal::new(false);
    let highlights_all: RwSignal<Vec<HighlightNoteSummary>> = RwSignal::new(Vec::new());
    let highlights_loading = RwSignal::new(false);

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
    let catalog_view = Memo::new(move |_| {
        shared.preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| pref_str(p, "booksCatalogView"))
                .unwrap_or("grid")
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

    // Load Highlights summary lazily when visiting Highlights or after reload.
    Effect::new(move |_| {
        let _ = reload.get();
        let is_hl = view.get() == View::Highlights;
        if is_hl || highlights_all.with_untracked(Vec::is_empty) {
            if is_hl {
                highlights_loading.set(true);
            }
            spawn_local(async move {
                let list = highlights::load_all_summary().await;
                let _ = highlights_all.try_set(list);
                let _ = highlights_loading.try_set(false);
            });
        }
    });

    // Poll every 3s while background book metadata indexing is active.
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

    let save_prefs = Callback::new(move |patch: Map<String, Value>| {
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

    let upload_files = Callback::new(move |files: web_sys::FileList| {
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
    });

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
                .run(move || confirm.set(Some(vec![t_trash.clone()]))),
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
                View::Home | View::All | View::Series | View::Highlights => lib.books.clone(),
            }
        })
    });

    let visible_count = Memo::new(move |_| {
        let base = filtered_list.get();
        if view.get() == View::Series {
            return crate::books::model::group_series(&base).len();
        }
        let f = filter.get();
        filter_books(&base, &f).len()
    });

    let highlights_count = Signal::derive(move || {
        highlights_all.with(|list| list.iter().map(|g| g.highlights.len()).sum::<usize>())
    });

    let choose = Callback::new(move |next: View| {
        if next != view.get_untracked() {
            filter.set(CatalogFilter::All);
            checked.set(HashSet::new());
            selecting.set(false);
        }
        view.set(next);
        navigation.set(false);
    });

    let trigger_upload = Callback::new(move |_| {
        if let Some(input) = file_input.get_untracked() {
            input.click();
        }
    });

    let open_book = Callback::new(move |id: String| {
        selected.set(Some(id.clone()));
        reading.set(Some(id));
    });

    let open_book_page = Callback::new(move |(id, page): (String, usize)| {
        deep_page.set_value(Some((id.clone(), page)));
        selected.set(Some(id.clone()));
        reading.set(Some(id));
    });

    let open_series_filter = Callback::new(move |series_name: String| {
        view.set(View::All);
        filter.set(CatalogFilter::SeriesName(series_name));
        let mut map = Map::new();
        map.insert("booksCatalogView".into(), json!("grid"));
        save_prefs.run(map);
    });

    let on_bulk_status = Callback::new(move |next_status: &'static str| {
        let ids: HashSet<String> = checked.get_untracked();
        if ids.is_empty() {
            return;
        }
        let targets: Vec<BookItem> = library.with_untracked(|l| {
            l.as_ref()
                .map(|l| {
                    l.books
                        .iter()
                        .filter(|b| ids.contains(&b.id))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        });
        let count = targets.len();
        for b in targets {
            set_status.run((b, next_status, false));
        }
        checked.set(HashSet::new());
        selecting.set(false);
        notice.set(Some(Notice {
            message: format!(
                "Updated {count} {} to {}",
                if count == 1 { "book" } else { "books" },
                status_label(next_status).to_lowercase()
            ),
            undo: None,
        }));
    });

    let on_bulk_trash = Callback::new(move |_| {
        let ids: HashSet<String> = checked.get_untracked();
        let targets: Vec<BookItem> = library.with_untracked(|l| {
            l.as_ref()
                .map(|l| {
                    l.books
                        .iter()
                        .filter(|b| ids.contains(&b.id))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        });
        if !targets.is_empty() {
            confirm.set(Some(targets));
        }
    });

    let on_close_reader = Callback::new(move |_| {
        reading.set(None);
        reload.update(|n| *n += 1);
    });

    let shell_class = move || {
        format!(
            "app-shell pim-shell books-shell{}{}{}",
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
            if dragging.get() { " is-dragging" } else { "" },
        )
    };

    view! {
        <div
            class=shell_class
            on:dragover=move |ev: web_sys::DragEvent| {
                if open_id.with_untracked(Option::is_some) {
                    return;
                }
                ev.prevent_default();
                dragging.set(true);
            }
            on:dragleave=move |ev: web_sys::DragEvent| {
                if ev.related_target().is_none() {
                    dragging.set(false);
                }
            }
            on:drop=move |ev: web_sys::DragEvent| {
                ev.prevent_default();
                dragging.set(false);
                if open_id.with_untracked(Option::is_some) {
                    return;
                }
                if let Some(dt) = ev.data_transfer()
                    && let Some(files) = dt.files()
                {
                    upload_files.run(files);
                }
            }
        >
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
                <BooksSidebar
                    navigation_id=navigation_id.clone()
                    navigation_ref=navigation_ref
                    file_input=file_input
                    navigation=navigation
                    library=library
                    view=view
                    highlights_count=highlights_count
                    on_choose=choose
                    on_trigger_upload=trigger_upload
                    on_upload_files=upload_files
                    on_settings=Callback::new(move |_| settings.set(true))
                />

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

                    let is_catalog_view = move || {
                        !matches!(view.get(), View::Home | View::Highlights)
                    };

                    view! {
                        <main class="pim-main books-main" tabindex="-1" aria-label=move || title.get()>
                            <Show when=move || dragging.get()>
                                <div class="books-drop-overlay" aria-hidden="true">
                                    <Icon name="upload"/>
                                    <strong>"Drop books to add them to your library"</strong>
                                    <span>"EPUB, PDF, CBZ, CBR, CB7 and CBT"</span>
                                </div>
                            </Show>
                            <div class=move || if is_catalog_view() { "books-heading-bar has-subbar" } else { "books-heading-bar" }>
                                <h1>{move || title.get()}</h1>
                                {move || {
                                    let show_cnt = library.with(Option::is_some)
                                        && !matches!(view.get(), View::Home | View::Highlights);
                                    show_cnt.then(|| view! {
                                        <span class="books-heading-count">{move || visible_count.get()}</span>
                                    })
                                }}
                                <div class="books-tools">
                                    {move || {
                                        let cur_v = view.get();
                                        let has_books = library.with(|l| l.as_ref().is_some_and(|l| !l.books.is_empty()));
                                        (cur_v == View::Home && has_books).then(|| view! {
                                            <button
                                                type="button"
                                                class="secondary-button books-browse-btn"
                                                on:click=move |_| choose.run(View::All)
                                            >
                                                "Browse all"
                                            </button>
                                        })
                                    }}
                                    {move || {
                                        let cur_v = view.get();
                                        let show = !matches!(cur_v, View::Home | View::Highlights | View::Series)
                                            && library.with(|l| l.as_ref().is_some_and(|l| !l.books.is_empty()));
                                        show.then(|| view! {
                                            <div class="books-view-seg" role="group" aria-label="Catalog layout">
                                                {[("grid", "Covers"), ("list", "List"), ("series", "Series")].into_iter().map(|(val, lbl)| view! {
                                                    <button
                                                        type="button"
                                                        class=move || if catalog_view.get() == val { "is-active" } else { "" }
                                                        aria-pressed=move || (catalog_view.get() == val).to_string()
                                                        on:click=move |_| {
                                                            let mut map = Map::new();
                                                            map.insert("booksCatalogView".into(), json!(val));
                                                            save_prefs.run(map);
                                                        }
                                                    >{lbl}</button>
                                                }).collect_view()}
                                            </div>
                                        })
                                    }}
                                    {move || {
                                        let show = is_catalog_view()
                                            && view.get() != View::Series
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
                                let cur_v = view.get();
                                let show_subbar = !matches!(cur_v, View::Home | View::Highlights | View::Series)
                                    && library.with(|l| l.as_ref().is_some_and(|l| !l.books.is_empty()));
                                show_subbar.then(|| view! {
                                    <BooksFilterBar
                                        filter=filter
                                        selecting=selecting
                                        checked=checked
                                        total_visible=Signal::from(visible_count)
                                        on_bulk_status=on_bulk_status
                                        on_bulk_trash=on_bulk_trash
                                    />
                                })
                            }}
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
                                let cur_view = view.get();
                                if cur_view == View::Highlights {
                                    return view! {
                                        <BooksHighlightsView
                                            summaries=Signal::from(highlights_all)
                                            loading=Signal::from(highlights_loading)
                                            on_open_page=open_book_page
                                        />
                                    }.into_any();
                                }
                                if lib.books.is_empty() {
                                    return view! {
                                        <div class="books-empty">
                                            <span class="empty-icon"><Icon name="book"/></span>
                                            <h2>{format!("No books in {} yet", lib.path)}</h2>
                                            <p>"Drop EPUB, PDF or comic files (CBZ, CBR) here, add them below, or move them into this folder in Files."</p>
                                            <div class="books-empty-actions">
                                                <button class="primary-button" on:click=move |_| trigger_upload.run(())><Icon name="upload"/>"Add books"</button>
                                                <a class="secondary-button" href=format!("/?folder={}", lib.root_id)>"Open in Files"</a>
                                            </div>
                                        </div>
                                    }.into_any();
                                }
                                let cur_sort = sort.get();
                                if cur_view == View::Home {
                                    return view! {
                                        <BooksHome
                                            lib=lib
                                            sort=cur_sort
                                            selected=selected
                                            on_select=Callback::new(move |id: String| selected.set(Some(id)))
                                            on_open=open_book
                                            on_choose=choose
                                            on_open_series=open_series_filter
                                            actions=Callback::new(move |b: BookItem| book_actions(b))
                                        />
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
                                                    <button class="primary-button" on:click=move |_| trigger_upload.run(())><Icon name="upload"/>"Add books"</button>
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

                                let mode = if cur_view == View::Series {
                                    "series".to_owned()
                                } else {
                                    catalog_view.get()
                                };
                                view! {
                                    <BooksCatalogGrid
                                        items=items
                                        sort=cur_sort
                                        filter=filter.get()
                                        catalog_view=mode
                                        grid_label=title.get_untracked()
                                        truncated=lib.truncated
                                        selected=selected
                                        selecting=selecting
                                        checked=checked
                                        on_select=Callback::new(move |id: String| selected.set(Some(id)))
                                        on_open_series=open_series_filter
                                        on_clear_filter=Callback::new(move |_| filter.set(CatalogFilter::All))
                                        actions=Callback::new(move |b: BookItem| book_actions(b))
                                    />
                                }.into_any()
                            }}
                        </main>
                    }
                    .into_any()
                }}

                {move || {
                    let id = details_id.get()?;
                    let id_read = id.clone();
                    let id_page = id.clone();
                    let id_status = id.clone();
                    let id_actions = id.clone();
                    Some(view! {
                        <aside class="pim-details books-details" tabindex="-1" aria-label="Book details">
                            <BookDetails
                                book_id=id
                                library=library
                                on_close=Callback::new(move |_| selected.set(None))
                                on_read=Callback::new(move |_| reading.set(Some(id_read.clone())))
                                on_open_page=Callback::new(move |page: usize| open_book_page.run((id_page.clone(), page)))
                                on_status=Callback::new(move |next_status: &'static str| {
                                    if let Some(b) = library.with_untracked(|l| l.as_ref().and_then(|l| l.books.iter().find(|b| b.id == id_status).cloned())) {
                                        set_status.run((b, next_status, true));
                                    }
                                })
                                on_select=Callback::new(move |next_id: String| selected.set(Some(next_id)))
                                on_filter_author=Callback::new(move |author: String| {
                                    view.set(View::All);
                                    filter.set(CatalogFilter::Author(author));
                                })
                                on_filter_tag=Callback::new(move |tag: String| {
                                    view.set(View::All);
                                    filter.set(CatalogFilter::Tag(tag));
                                })
                                on_filter_series=open_series_filter
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
                confirm.get().map(|targets| {
                    let title = if targets.len() == 1 {
                        "Move book to deleted files?".to_owned()
                    } else {
                        format!("Move {} books to deleted files?", targets.len())
                    };
                    let targets_sv = StoredValue::new(targets);
                    view! {
                        <Dialog
                            title=title
                            on_close=Callback::new(move |_| confirm.set(None))
                        >
                            {move || {
                                let list = targets_sv.get_value();
                                view! {
                                    <ConfirmTrash
                                        books=list
                                        live=live
                                        on_cancel=Callback::new(move |_| confirm.set(None))
                                        on_done=Callback::new(move |trashed: Vec<(String, String)>| {
                                            for (id, _) in &trashed {
                                                if selected.get_untracked().as_deref() == Some(id.as_str()) {
                                                    selected.set(None);
                                                }
                                                if reading.get_untracked().as_deref() == Some(id.as_str()) {
                                                    reading.set(None);
                                                }
                                            }
                                            checked.set(HashSet::new());
                                            selecting.set(false);
                                            confirm.set(None);
                                            let msg = if trashed.len() == 1 {
                                                format!("Moved \u{201c}{}\u{201d} to deleted files", trashed[0].1)
                                            } else {
                                                format!("Moved {} books to deleted files", trashed.len())
                                            };
                                            notice.set(Some(Notice { message: msg, undo: None }));
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
    books: Vec<BookItem>,
    live: Live,
    on_cancel: Callback<()>,
    on_done: Callback<Vec<(String, String)>>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let err = RwSignal::new(String::new());
    let books_sv = StoredValue::new(books);
    let on_confirm = move |_| {
        busy.set(true);
        let list = books_sv.get_value();
        spawn_local(async move {
            let Some(conn) = live.connection.get_untracked() else {
                let _ = err.try_set("Books is reconnecting. Try again in a moment.".into());
                let _ = busy.try_set(false);
                return;
            };
            let mut done = Vec::with_capacity(list.len());
            for b in list {
                let id = b.id.clone();
                match reducer(|tx| conn.reducers.trash_entry_then(id, b.revision, replied(tx)))
                    .await
                {
                    Ok(()) => done.push((b.id, b.title)),
                    Err(msg) => {
                        let _ = err.try_set(msg);
                        let _ = busy.try_set(false);
                        return;
                    }
                }
            }
            let _ = on_done.try_run(done);
        });
    };
    let text = books_sv.with_value(|list| {
        if list.len() == 1 {
            let b = &list[0];
            format!(
                "\u{201c}{}\u{201d} ({}) moves to Files \u{203a} Deleted files, where it can be restored. Your reading progress is kept.",
                b.title, b.name
            )
        } else {
            format!(
                "{} books will move to Files \u{203a} Deleted files, where they can be restored. Your reading progress is kept.",
                list.len()
            )
        }
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
