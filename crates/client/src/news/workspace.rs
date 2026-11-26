//! News workspace shell: coordinates sidebar, chronological list/cards, editorial reader,
//! panes, background polling, scroll/infinite observers, and keyboard shortcuts.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use super::list::NewsList;
use super::model::{
    ListFilter, NewsFeed, NewsFolder, NewsState, Pane, View, read_view, write_view,
};
use super::panes::{ConfirmAction, FeedPane, FolderPane, SubscribePane};
use super::reader::NewsReader;
use super::settings::NewsSettings;
use super::sidebar::NewsSidebar;
use crate::api::BrowserSession;
use crate::settings::Notice;
use crate::shared::context;
use crate::shell::WorkspaceHeader;
use crate::ui::{active_element, document, listen, unique_id, window};

#[component]
pub fn NewsWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let (start_view, start_item) = read_view();
    let shared_prefs = context().preferences;
    let pref_str = move |key: &'static str, default: &'static str| {
        Memo::new(move |_| {
            shared_prefs.value.with(|p| {
                p.as_ref()
                    .and_then(|p| p.rest.get(key))
                    .and_then(Value::as_str)
                    .unwrap_or(default)
                    .to_owned()
            })
        })
    };
    let news_mark_read = pref_str("newsMarkRead", "open");
    let news_order = pref_str("newsOrder", "newest");
    let news_images = pref_str("newsImages", "proxy");
    let news_layout = pref_str("newsLayout", "split");
    let news_paper = pref_str("newsPaper", "dar");
    let news_font = pref_str("newsFont", "serif");
    let news_size = pref_str("newsSize", "md");
    let news_show_read = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("newsShowRead"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
    });

    let state = NewsState {
        overview: RwSignal::new(None),
        load_error: RwSignal::new(String::new()),
        view_sig: RwSignal::new(start_view),
        items: RwSignal::new(Vec::new()),
        next_offset: RwSignal::new(None),
        list_state: RwSignal::new("loading"),
        list_error: RwSignal::new(String::new()),
        filter: RwSignal::new(ListFilter::All),
        checked: RwSignal::new(HashSet::new()),
        sidebar_unread_only: RwSignal::new(false),
        focus_mode: RwSignal::new(false),
        aa_open: RwSignal::new(false),
        feed_info: RwSignal::new(None),
        selected: RwSignal::new(start_item),
        article: RwSignal::new(None),
        article_error: RwSignal::new(String::new()),
        fulltext_loading: RwSignal::new(false),
        fulltext_fetched: RwSignal::new(None),
        pane: RwSignal::new(None),
        confirm: RwSignal::new(None),
        notice: RwSignal::new(None),
        busy: RwSignal::new(false),
        refreshing: RwSignal::new(false),
        navigation: RwSignal::new(false),
        list_ref: NodeRef::<html::Div>::new(),
        generation: RwSignal::new(0),
        pending_read: StoredValue::new(HashSet::new()),
        session: StoredValue::new(session.clone()),
        news_mark_read,
        news_order,
        news_show_read,
        news_images,
        news_layout,
        news_paper,
        news_font,
        news_size,
    };

    // Mobile navigation drawer focus & Escape handling.
    let navigation_id = unique_id();
    let navigation_ref = NodeRef::<html::Aside>::new();
    let nav_trigger = StoredValue::new_local(None::<web_sys::HtmlElement>);
    let nav_was_open = RwSignal::new(false);
    Effect::new(move |_| {
        let open = state.navigation.get();
        if open {
            nav_trigger.set_value(active_element());
            if let Some(el) = navigation_ref
                .get()
                .and_then(|r| r.query_selector("button,a").ok().flatten())
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            {
                let _ = el.focus();
            }
            listen(&document(), "keydown", false, move |event| {
                let event: web_sys::KeyboardEvent = event.unchecked_into();
                if event.key() == "Escape" && !event.default_prevented() {
                    event.prevent_default();
                    state.navigation.set(false);
                }
            });
        } else if nav_was_open.get_untracked() {
            let active = document().active_element();
            let body_active = document().body().is_some_and(|b| {
                active
                    .as_ref()
                    .is_some_and(|a| a.unchecked_ref::<web_sys::Node>() == b.as_ref())
            });
            let inside_nav = navigation_ref.get_untracked().is_some_and(|r| {
                r.contains(active.as_ref().map(|a| a.unchecked_ref::<web_sys::Node>()))
            });
            if body_active || inside_nav {
                nav_trigger.with_value(|t| {
                    if let Some(el) = t {
                        let _ = el.focus();
                    }
                });
            }
        }
        nav_was_open.set(open);
    });

    // Close Aa typography popover on outside click.
    listen(&document(), "click", false, move |_| {
        if state.aa_open.try_get_untracked() == Some(true) {
            let _ = state.aa_open.try_set(false);
        }
    });

    // Overview initial load and 60-second background poll.
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }
    spawn_local(async move {
        state.fetch_overview().await;
        if let View::Feed(id) = start_view {
            state.load_feed_info(id);
        }
        while alive.load(Ordering::Relaxed) {
            crate::sleep(60_000).await;
            if !alive.load(Ordering::Relaxed) {
                break;
            }
            state.fetch_overview().await;
        }
    });

    // Reload items whenever view or ordering/read preferences change.
    Effect::new(move |_| {
        state.view_sig.track();
        state.news_order.track();
        state.news_show_read.track();
        state.load_items();
    });

    // Keep URL query synchronized with current view and selected article.
    Effect::new(move |_| {
        write_view(state.view_sig.get(), state.selected.get());
    });

    // Open initial item from URL query if present.
    if let Some(id) = start_item {
        state.open(id);
    }

    // Auto-dismiss toasts after 8 seconds.
    let notice_seq = RwSignal::new(0u32);
    Effect::new(move |_| {
        if state.notice.with(Option::is_none) {
            return;
        }
        let id = notice_seq.get_untracked() + 1;
        notice_seq.set(id);
        spawn_local(async move {
            crate::sleep(8000).await;
            if notice_seq.try_get_untracked() == Some(id) {
                let _ = state.notice.try_set(None);
            }
        });
    });

    // Mark on scroll: rows whose bottom passes the list's top edge count as read.
    let scroll_seq = RwSignal::new(0u32);
    Effect::new(move |_| {
        state.items.track();
        state.filter.track();
        state.news_layout.track();
        if state.news_mark_read.get() != "scroll" {
            return;
        }
        let Some(root) = state.list_ref.get() else {
            return;
        };
        let init = web_sys::IntersectionObserverInit::new();
        let root_el: &web_sys::Element = &root;
        init.set_root(Some(root_el));
        init.set_threshold(&JsValue::from_f64(0.0));
        let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            let mut added = false;
            for idx in 0..entries.length() {
                let Ok(entry) = entries
                    .get(idx)
                    .dyn_into::<web_sys::IntersectionObserverEntry>()
                else {
                    continue;
                };
                if !entry.is_intersecting()
                    && let Some(bounds) = entry.root_bounds()
                    && entry.bounding_client_rect().bottom() <= bounds.top() + 1.0
                    && let Some(id) = entry
                        .target()
                        .get_attribute("data-item")
                        .and_then(|v| v.parse::<i64>().ok())
                {
                    let is_unread = state.items.with_untracked(|list| {
                        list.iter().find(|i| i.id == id).is_some_and(|i| i.unread)
                    });
                    if is_unread {
                        state.pending_read.update_value(|set| {
                            set.insert(id);
                        });
                        added = true;
                    }
                }
            }
            if added || state.pending_read.with_value(|s| !s.is_empty()) {
                let seq = scroll_seq.get_untracked() + 1;
                scroll_seq.set(seq);
                spawn_local(async move {
                    crate::sleep(600).await;
                    if scroll_seq.try_get_untracked() != Some(seq) {
                        return;
                    }
                    let ids: Vec<i64> = state
                        .pending_read
                        .try_update_value(|set| set.drain().collect())
                        .unwrap_or_default();
                    let targets: Vec<(i64, i64)> = state.items.with_untracked(|list| {
                        ids.into_iter()
                            .filter_map(|id| {
                                list.iter()
                                    .find(|i| i.id == id && i.unread)
                                    .map(|i| (i.id, i.feed_id))
                            })
                            .collect()
                    });
                    state.set_state(targets, Some(false), None);
                });
            }
        });
        let Ok(observer) = web_sys::IntersectionObserver::new_with_options(
            callback.as_ref().unchecked_ref(),
            &init,
        ) else {
            return;
        };
        if let Ok(rows) = root.query_selector_all("[data-item]") {
            for i in 0..rows.length() {
                if let Some(el) = rows
                    .item(i)
                    .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
                {
                    observer.observe(&el);
                }
            }
        }
        let kept = send_wrapper::SendWrapper::new((observer, callback));
        on_cleanup(move || {
            let _ = scroll_seq.try_update(|s| *s += 1);
            let (observer, _cb) = kept.take();
            observer.disconnect();
        });
    });

    // Infinite scroll sentinel for `.news-more`.
    Effect::new(move |_| {
        state.items.track();
        state.next_offset.track();
        state.list_state.track();
        let Some(root) = state.list_ref.get() else {
            return;
        };
        let Some(sentinel) = root.query_selector(".news-more").ok().flatten() else {
            return;
        };
        let init = web_sys::IntersectionObserverInit::new();
        let root_el: &web_sys::Element = &root;
        init.set_root(Some(root_el));
        let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            let intersecting = (0..entries.length()).any(|idx| {
                entries
                    .get(idx)
                    .dyn_into::<web_sys::IntersectionObserverEntry>()
                    .is_ok_and(|e| e.is_intersecting())
            });
            if intersecting {
                state.load_more();
            }
        });
        let Ok(observer) = web_sys::IntersectionObserver::new_with_options(
            callback.as_ref().unchecked_ref(),
            &init,
        ) else {
            return;
        };
        observer.observe(&sentinel);
        let kept = send_wrapper::SendWrapper::new((observer, callback));
        on_cleanup(move || {
            let (observer, _cb) = kept.take();
            observer.disconnect();
        });
    });

    // Keyboard shortcuts work outside fields, dialogs, menus and editors.
    listen(&document(), "keydown", false, move |event| {
        let event: web_sys::KeyboardEvent = event.unchecked_into();
        if event.default_prevented() || event.ctrl_key() || event.meta_key() || event.alt_key() {
            return;
        }
        let blocked = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| {
                el.closest("input,textarea,select,[contenteditable],[role=dialog],[role=menu],.editing-pane")
                    .ok()
                    .flatten()
            })
            .is_some();
        if blocked {
            return;
        }
        let sel = state.selected.get_untracked();
        let current = sel.and_then(|sel_id| {
            state
                .article
                .get_untracked()
                .filter(|a| a.id == sel_id)
                .map(|a| (a.id, a.feed_id, a.unread, a.starred, a.url))
                .or_else(|| {
                    state
                        .items
                        .get_untracked()
                        .into_iter()
                        .find(|i| i.id == sel_id)
                        .map(|i| (i.id, i.feed_id, i.unread, i.starred, i.url))
                })
        });
        let key = event.key();
        match key.as_str() {
            "j" | "n" => {
                event.prevent_default();
                state.step(1, key == "n");
            }
            "k" | "p" => {
                event.prevent_default();
                state.step(-1, false);
            }
            "s" => {
                if let Some((id, feed_id, _, starred, _)) = current {
                    event.prevent_default();
                    state.set_state(vec![(id, feed_id)], None, Some(!starred));
                }
            }
            "m" => {
                if let Some((id, feed_id, unread, _, _)) = current {
                    event.prevent_default();
                    state.set_state(vec![(id, feed_id)], Some(!unread), None);
                }
            }
            "o" => {
                if let Some((_, _, _, _, url)) = current
                    && !url.is_empty()
                {
                    event.prevent_default();
                    let _ = window().open_with_url_and_target_and_features(
                        &url,
                        "_blank",
                        "noopener,noreferrer",
                    );
                }
            }
            "f" => {
                if sel.is_some() {
                    event.prevent_default();
                    state.focus_mode.update(|v| *v = !*v);
                }
            }
            "a" => {
                if sel.is_some() {
                    event.prevent_default();
                    state.aa_open.update(|v| *v = !*v);
                }
            }
            "v" => {
                event.prevent_default();
                let next = if state.news_layout.get_untracked() == "cards" {
                    "split"
                } else {
                    "cards"
                };
                if next == "cards" {
                    state.selected.set(None);
                    state.article.set(None);
                    state.focus_mode.set(false);
                }
                state.save_pref("newsLayout", json!(next));
            }
            "r" => {
                event.prevent_default();
                state.refresh();
            }
            "A" if event.shift_key() => {
                event.prevent_default();
                state.mark_all();
            }
            "Escape" => {
                if state.aa_open.get_untracked() {
                    event.prevent_default();
                    state.aa_open.set(false);
                } else if state.focus_mode.get_untracked() {
                    event.prevent_default();
                    state.focus_mode.set(false);
                } else if sel.is_some() {
                    event.prevent_default();
                    state.selected.set(None);
                    state.article.set(None);
                }
            }
            _ => {}
        }
    });

    let title = Memo::new(move |_| match state.view_sig.get() {
        View::Feed(id) => state.overview.with(|o| {
            o.as_ref()
                .and_then(|o| o.feeds.iter().find(|f| f.id == id))
                .map(|f| f.title.clone())
                .unwrap_or_else(|| "Feed".into())
        }),
        View::Folder(id) => state.overview.with(|o| {
            o.as_ref()
                .and_then(|o| o.folders.iter().find(|f| f.id == id))
                .map(|f| f.name.clone())
                .unwrap_or_else(|| "Folder".into())
        }),
        View::Today => "Today".into(),
        View::Starred => "Starred".into(),
        View::Media => "Podcasts & media".into(),
        View::All => "All articles".into(),
        View::Unread => "Unread".into(),
    });

    let reader_item = Signal::derive(move || {
        let sel = state.selected.get()?;
        state.article.get().filter(|a| a.id == sel)
    });

    let reader_feed = Signal::derive(move || {
        let feed_id = reader_item.with(|a| a.as_ref().map(|a| a.feed_id))?;
        state.overview.with(|o| {
            o.as_ref()
                .and_then(|o| o.feeds.iter().find(|f| f.id == feed_id).cloned())
        })
    });

    let show_reader = Memo::new(move |_| {
        state.selected.get().is_some() || state.news_layout.get() != "cards"
    });

    view! {
        <div class=move || {
            let mut cls = String::from("app-shell pim-shell news-shell");
            if state.selected.get().is_some() {
                cls.push_str(" has-pim-details");
            }
            if state.news_layout.get() != "cards" {
                cls.push_str(" has-news-split");
            }
            if state.focus_mode.get() && state.selected.get().is_some() {
                cls.push_str(" is-news-focus");
            }
            cls
        }>
            <WorkspaceHeader
                app="news"
                session=session.clone()
                on_signed_out=on_signed_out
                on_settings=Callback::new(move |_| state.pane.set(Some(Pane::Settings)))
            />
            <div class="pim-workspace news-workspace">
                {move || state.navigation.get().then(|| view! {
                    <button
                        type="button"
                        class="nav-scrim"
                        aria-label="Close navigation"
                        on:click=move |_| state.navigation.set(false)
                    ></button>
                })}
                <NewsSidebar
                    state=state
                    navigation_id=navigation_id
                    navigation_ref=navigation_ref
                />
                <NewsList state=state title=title/>
                <Show when=move || show_reader.get()>
                    <aside class="pim-details news-details" tabindex="-1" aria-label="Article">
                        <NewsReader state=state item=reader_item feed=reader_feed/>
                    </aside>
                </Show>
            </div>

            {move || state.pane.get().map(|p| {
                let folders = state
                    .overview
                    .with_untracked(|o| o.as_ref().map(|o| o.folders.clone()).unwrap_or_default());
                let sess = state.session.get_value();
                match p {
                    Pane::Subscribe => view! {
                        <SubscribePane
                            folders=folders
                            csrf=sess.csrf_token
                            on_close=Callback::new(move |_| state.pane.set(None))
                            on_done=Callback::new(move |feed: NewsFeed| {
                                state.pane.set(None);
                                spawn_local(async move {
                                    state.fetch_overview().await;
                                    state.choose(View::Feed(feed.id));
                                    let _ = state.notice.try_set(Some(Notice {
                                        message: format!("Subscribed to {}", feed.title),
                                        undo: None,
                                    }));
                                });
                            })
                            on_settings=Callback::new(move |_| state.pane.set(Some(Pane::Settings)))
                        />
                    }.into_any(),
                    Pane::Feed(feed) => {
                        let current = state
                            .overview
                            .with_untracked(|o| {
                                o.as_ref()
                                    .and_then(|o| o.feeds.iter().find(|f| f.id == feed.id).cloned())
                            })
                            .unwrap_or(feed);
                        view! {
                            <FeedPane
                                feed=current
                                folders=folders
                                csrf=sess.csrf_token
                                on_close=Callback::new(move |_| state.pane.set(None))
                                on_saved=Callback::new(move |_| {
                                    state.pane.set(None);
                                    spawn_local(async move {
                                        state.fetch_overview().await;
                                        state.fetch_items().await;
                                        state.notify("Feed saved");
                                    });
                                })
                            />
                        }.into_any()
                    }
                    Pane::Folder(folder) => {
                        let is_new = folder.is_none();
                        view! {
                            <FolderPane
                                folder=folder
                                csrf=sess.csrf_token
                                on_close=Callback::new(move |_| state.pane.set(None))
                                on_saved=Callback::new(move |saved_folder: NewsFolder| {
                                    state.pane.set(None);
                                    spawn_local(async move {
                                        state.fetch_overview().await;
                                        if is_new {
                                            state.choose(View::Folder(saved_folder.id));
                                        }
                                    });
                                })
                            />
                        }.into_any()
                    }
                    Pane::Settings => view! {
                        <NewsSettings
                            session=sess
                            on_close=Callback::new(move |_| state.pane.set(None))
                            on_imported=Callback::new(move |_| {
                                spawn_local(async move {
                                    state.fetch_overview().await;
                                    state.fetch_items().await;
                                });
                            })
                        />
                    }.into_any(),
                }
            })}

            {move || state.confirm.get().map(|spec| view! {
                <ConfirmAction spec=spec on_close=Callback::new(move |_| state.confirm.set(None))/>
            })}

            <div class="news-toast-region" role="status" aria-live="polite">
                {move || state.notice.get().map(|n| view! {
                    <div class="settings-toast news-toast">
                        <span>{n.message}</span>
                        {n.undo.map(|undo| view! {
                            <button
                                type="button"
                                class="text-button"
                                on:click=move |_| {
                                    state.notice.set(None);
                                    undo();
                                }
                            >
                                "Undo"
                            </button>
                        })}
                    </div>
                })}
            </div>
        </div>
    }
}
