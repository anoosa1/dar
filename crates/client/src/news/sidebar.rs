//! News sidebar: Subscribe CTA, smart views (Today, Unread, Starred, Podcasts & media,
//! All articles), Pinned feeds shelf, collapsible Folders & Feeds with an Unread-only filter,
//! failing-feed health pill, and sync status footer.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use super::model::{BASE, NewsFeed, NewsState, Pane, View, initial, mark_class, relative};
use crate::api;
use crate::icons::Icon;
use crate::ui::ActionMenu;

#[component]
pub fn NewsSidebar(
    state: NewsState,
    navigation_id: String,
    navigation_ref: NodeRef<html::Aside>,
) -> impl IntoView {
    let nav_views: [(View, &'static str, &'static str); 5] = [
        (View::Today, "Today", "live"),
        (View::Unread, "Unread", "inbox"),
        (View::Starred, "Starred", "star"),
        (View::Media, "Podcasts & media", "headphones"),
        (View::All, "All articles", "file"),
    ];

    let feed_row = move |feed: NewsFeed, nested: bool, show_pin: bool| {
        let feed_id = feed.id;
        let feed_title = feed.title.clone();
        let feed_menu = feed.clone();
        let paused = feed.paused;
        let pinned = feed.pinned;
        let err_count = feed.update_error_count;
        let last_err = feed.last_update_error.clone();
        let unread_count = feed.unread_count;
        let active = move || state.view_sig.get() == View::Feed(feed_id);
        let row_class = if nested {
            "news-row news-feed-row is-nested"
        } else {
            "news-row news-feed-row"
        };
        let menu_label = format!("Actions for {feed_title}");
        view! {
            <div class=row_class>
                <button
                    type="button"
                    class=move || format!(
                        "nav-item{}{}",
                        if active() { " active" } else { "" },
                        if paused { " is-paused" } else { "" }
                    )
                    aria-current=move || active().then_some("page")
                    on:click=move |_| state.choose(View::Feed(feed_id))
                >
                    <span class=mark_class(feed_id) aria-hidden="true">{initial(&feed_title)}</span>
                    <span class="news-row-title">{feed_title.clone()}</span>
                    {(show_pin && pinned).then(|| view! {
                        <span class="news-pin-mark" title="Pinned">
                            <Icon name="pin"/>
                            <span class="sr-only">"Pinned"</span>
                        </span>
                    })}
                    {(err_count > 0).then(|| view! {
                        <span class="news-warning" title=last_err.clone()>
                            <Icon name="alert"/>
                            <span class="sr-only">"Updates failing"</span>
                        </span>
                    })}
                    {paused.then(|| view! {
                        <span class="news-paused" title="Updates paused">
                            <Icon name="pause"/>
                            <span class="sr-only">"Paused"</span>
                        </span>
                    })}
                    {(unread_count > 0).then(|| view! {
                        <span class="news-count">{unread_count}<span class="sr-only">" unread"</span></span>
                    })}
                </button>
                <ActionMenu
                    label=menu_label
                    class="news-row-menu"
                    actions=move |_| state.feed_actions(feed_menu.clone())
                >
                    <Icon name="more"/>
                </ActionMenu>
            </div>
        }
    };

    view! {
        <aside
            id=navigation_id
            node_ref=navigation_ref
            class=move || if state.navigation.get() { "sidebar news-sidebar is-open" } else { "sidebar news-sidebar" }
            aria-label="News navigation"
        >
            <button
                type="button"
                class="new-button ui-new"
                on:click=move |_| {
                    state.navigation.set(false);
                    state.pane.set(Some(Pane::Subscribe));
                }
            >
                <Icon name="plus"/>"Subscribe"
            </button>

            <div class="news-sidebar-scroll">
                <nav aria-label="News views">
                    {nav_views.into_iter().map(|(target, label, icon)| {
                        let active = move || state.view_sig.get() == target;
                        let n = move || match target {
                            View::Unread => state.overview.with(|o| o.as_ref().map(|o| o.unread_count).unwrap_or(0)),
                            View::Starred => state.overview.with(|o| o.as_ref().map(|o| o.starred_count).unwrap_or(0)),
                            _ => 0,
                        };
                        view! {
                            <button
                                type="button"
                                class=move || format!("nav-item news-view{}", if active() { " active" } else { "" })
                                aria-current=move || active().then_some("page")
                                on:click=move |_| state.choose(target)
                            >
                                <Icon name=icon/>
                                <span class="news-row-title">{label}</span>
                                {move || (n() > 0).then(|| view! { <span class="news-count">{n()}</span> })}
                            </button>
                        }
                    }).collect_view()}
                </nav>

                {move || {
                    let pinned: Vec<NewsFeed> = state.overview.with(|o| {
                        o.as_ref()
                            .map(|o| o.feeds.iter().filter(|f| f.pinned).cloned().collect())
                            .unwrap_or_default()
                    });
                    (!pinned.is_empty()).then(|| view! {
                        <div class="sidebar-heading news-heading">
                            <h2>"Pinned"</h2>
                        </div>
                        <nav aria-label="Pinned feeds" class="news-feeds">
                            {pinned.into_iter().map(|f| feed_row(f, false, true)).collect_view()}
                        </nav>
                    })
                }}

                <div class="sidebar-heading news-heading">
                    <h2>"Folders"</h2>
                    <div class="news-heading-actions">
                        <button
                            type="button"
                            class=move || if state.sidebar_unread_only.get() { "news-side-chip is-active" } else { "news-side-chip" }
                            aria-pressed=move || state.sidebar_unread_only.get().to_string()
                            title="Show only feeds with unread articles"
                            on:click=move |_| state.sidebar_unread_only.update(|v| *v = !*v)
                        >
                            "Unread"
                        </button>
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="New folder"
                            title="New folder"
                            on:click=move |_| {
                                state.navigation.set(false);
                                state.pane.set(Some(Pane::Folder(None)));
                            }
                        >
                            <Icon name="plus"/>
                        </button>
                    </div>
                </div>

                <nav aria-label="Feeds" class="news-feeds">
                    {move || {
                        let Some(ov) = state.overview.get() else {
                            return Vec::<AnyView>::new().into_any();
                        };
                        let unread_only = state.sidebar_unread_only.get();
                        let folders_view = ov.folders.iter().filter_map(|folder| {
                            let folder_id = folder.id;
                            let folder_name = folder.name.clone();
                            let opened = folder.opened;
                            let folder_menu = folder.clone();
                            let all_children: Vec<NewsFeed> = ov
                                .feeds
                                .iter()
                                .filter(|f| f.folder_id == folder_id)
                                .cloned()
                                .collect();
                            let unread: i64 = all_children.iter().map(|f| f.unread_count).sum();
                            let any_failing = all_children.iter().any(|f| f.update_error_count > 0);
                            if unread_only && unread == 0 && !any_failing {
                                return None;
                            }
                            let visible_children: Vec<NewsFeed> = if unread_only {
                                all_children
                                    .into_iter()
                                    .filter(|f| f.unread_count > 0 || f.update_error_count > 0)
                                    .collect()
                            } else {
                                all_children
                            };
                            let active = move || state.view_sig.get() == View::Folder(folder_id);
                            let toggle_label = if opened {
                                format!("Collapse {folder_name}")
                            } else {
                                format!("Expand {folder_name}")
                            };
                            let menu_label = format!("Actions for folder {folder_name}");
                            Some(view! {
                                <div class="news-row news-folder-row">
                                    <button
                                        type="button"
                                        class="icon-button news-toggle"
                                        aria-label=toggle_label
                                        aria-expanded=opened.to_string()
                                        on:click=move |_| {
                                            let next = !opened;
                                            state.overview.update(|o| {
                                                if let Some(o) = o {
                                                    for f in &mut o.folders {
                                                        if f.id == folder_id {
                                                            f.opened = next;
                                                        }
                                                    }
                                                }
                                            });
                                            let csrf = state.csrf();
                                            spawn_local(async move {
                                                if let Err(e) = api::send::<Value>(
                                                    &format!("{BASE}/folders/{folder_id}"),
                                                    "PATCH",
                                                    Some(json!({ "opened": next })),
                                                    &csrf,
                                                )
                                                .await
                                                {
                                                    state.fail(e.message);
                                                }
                                            });
                                        }
                                    >
                                        <Icon name="chevron"/>
                                    </button>
                                    <button
                                        type="button"
                                        class=move || format!("nav-item{}", if active() { " active" } else { "" })
                                        aria-current=move || active().then_some("page")
                                        on:click=move |_| state.choose(View::Folder(folder_id))
                                    >
                                        <Icon name="folder"/>
                                        <span class="news-row-title">{folder_name.clone()}</span>
                                        {any_failing.then(|| view! {
                                            <span class="news-warning">
                                                <Icon name="alert"/>
                                                <span class="sr-only">"A feed is failing"</span>
                                            </span>
                                        })}
                                        {(unread > 0).then(|| view! { <span class="news-count">{unread}</span> })}
                                    </button>
                                    <ActionMenu
                                        label=menu_label
                                        class="news-row-menu"
                                        actions=move |_| state.folder_actions(folder_menu.clone())
                                    >
                                        <Icon name="more"/>
                                    </ActionMenu>
                                </div>
                                {opened.then(|| {
                                    visible_children
                                        .into_iter()
                                        .map(|feed| feed_row(feed, true, false))
                                        .collect_view()
                                })}
                            })
                        }).collect_view();

                        let unfiled = ov
                            .feeds
                            .into_iter()
                            .filter(|f| {
                                f.folder_id == 0
                                    && (!unread_only || f.unread_count > 0 || f.update_error_count > 0)
                            })
                            .map(|feed| feed_row(feed, false, false))
                            .collect_view();

                        view! { {folders_view}{unfiled} }.into_any()
                    }}
                </nav>
            </div>

            {move || state.overview.get().map(|ov| {
                let updated = ov.feeds.iter().map(|f| f.last_updated).fold(0.0f64, f64::max);
                let failing: Vec<NewsFeed> = ov
                    .feeds
                    .iter()
                    .filter(|f| f.update_error_count > 0)
                    .cloned()
                    .collect();
                let fail_count = failing.len();
                let first_fail = failing.first().map(|f| f.id);
                let has_feeds = !ov.feeds.is_empty();
                view! {
                    <div class="sidebar-footer news-footer">
                        {(fail_count > 0).then(|| view! {
                            <button
                                type="button"
                                class="news-health-pill"
                                on:click=move |_| {
                                    if let Some(fid) = first_fail {
                                        state.choose(View::Feed(fid));
                                    }
                                }
                            >
                                <Icon name="alert"/>
                                <span>
                                    {format!(
                                        "{fail_count} feed{} need{} attention",
                                        if fail_count == 1 { "" } else { "s" },
                                        if fail_count == 1 { "s" } else { "" }
                                    )}
                                </span>
                            </button>
                        })}
                        <button
                            type="button"
                            class="text-button news-sync-status"
                            disabled=move || state.refreshing.get()
                            on:click=move |_| state.refresh()
                        >
                            <span
                                class=move || {
                                    if state.refreshing.get() {
                                        "news-sync-dot is-busy"
                                    } else if fail_count > 0 {
                                        "news-sync-dot is-warn"
                                    } else if has_feeds {
                                        "news-sync-dot is-ok"
                                    } else {
                                        "news-sync-dot"
                                    }
                                }
                                aria-hidden="true"
                            ></span>
                            <span class="news-sync-label">
                                {move || if state.refreshing.get() {
                                    "Updating\u{2026}".to_owned()
                                } else if updated > 0.0 {
                                    format!("Updated {}", relative(updated))
                                } else {
                                    "Update feeds".to_owned()
                                }}
                            </span>
                            <Icon name="refresh"/>
                        </button>
                    </div>
                }
            })}

            <button
                type="button"
                class="nav-item ui-app-settings-entry"
                on:click=move |_| {
                    state.navigation.set(false);
                    state.pane.set(Some(Pane::Settings));
                }
            >
                <Icon name="settings"/>"News settings"
            </button>
        </aside>
    }
}
