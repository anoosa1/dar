//! News article list & Magazine Cards sheet: header (layout switch, refresh, mark all read,
//! feed/folder menu), tonal bulk selection bar, filter chips & inline sort, Feed Pulse
//! 14-day cadence card, failing feed banners, and chronologically day-grouped rows or cards.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use serde_json::json;

use super::model::{
    ListFilter, NewsItem, NewsState, Pane, View, day_label, initial, is_video_enclosure,
    iso_string, mark_class, proxy_image_url, time_label, visible_items,
};
use super::panes::FeedSparkline;
use crate::icons::Icon;
use crate::ui::ActionMenu;

#[component]
pub fn NewsList(state: NewsState, title: Memo<String>) -> impl IntoView {
    let visible = Memo::new(move |_| {
        let items = state.items.get();
        let view = state.view_sig.get();
        let filter = state.filter.get();
        let order = state.news_order.get();
        visible_items(&items, view, filter, &order)
    });

    let count = Memo::new(move |_| match state.view_sig.get() {
        View::Unread => state
            .overview
            .with(|o| o.as_ref().map(|o| o.unread_count)),
        View::Starred => state
            .overview
            .with(|o| o.as_ref().map(|o| o.starred_count)),
        View::Feed(id) => state.overview.with(|o| {
            o.as_ref()
                .and_then(|o| o.feeds.iter().find(|f| f.id == id))
                .map(|f| f.unread_count)
        }),
        View::Folder(id) => Some(state.overview.with(|o| {
            o.as_ref()
                .map(|o| {
                    o.feeds
                        .iter()
                        .filter(|f| f.folder_id == id)
                        .map(|f| f.unread_count)
                        .sum()
                })
                .unwrap_or(0)
        })),
        View::Today | View::Media => Some(visible.with(|v| v.len() as i64)),
        View::All => None,
    });

    let failing = Memo::new(move |_| {
        let v = state.view_sig.get();
        state.overview.with(|o| {
            o.as_ref()
                .map(|o| {
                    o.feeds
                        .iter()
                        .filter(|f| {
                            f.update_error_count > 0
                                && match v {
                                    View::Feed(id) => f.id == id,
                                    View::Folder(id) => f.folder_id == id,
                                    _ => false,
                                }
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });

    let is_selecting = Memo::new(move |_| !state.checked.with(HashSet::is_empty));
    let has_unread = Memo::new(move |_| visible.with(|list| list.iter().any(|i| i.unread)));

    let checked_targets = move || -> Vec<(i64, i64)> {
        let ids = state.checked.get_untracked();
        state.items.with_untracked(|list| {
            list.iter()
                .filter(|i| ids.contains(&i.id))
                .map(|i| (i.id, i.feed_id))
                .collect()
        })
    };

    let filters: [(ListFilter, &'static str, Option<&'static str>); 4] = [
        (ListFilter::All, "All", None),
        (ListFilter::Unread, "Unread", None),
        (ListFilter::Starred, "Starred", Some("star")),
        (ListFilter::Media, "Media", Some("headphones")),
    ];

    view! {
        <main class="pim-main news-main" tabindex="-1" aria-label=move || title.get()>
            <div class="news-list-heading">
                <h1>{move || title.get()}</h1>
                {move || count.get().map(|n| view! {
                    <span class="news-heading-count">
                        {n}
                        <span class="sr-only">
                            {if state.view_sig.get() == View::Starred { " starred" } else { " unread" }}
                        </span>
                    </span>
                })}
                <div class="news-tools">
                    <div class="news-layout-switch" role="group" aria-label="View layout">
                        <button
                            type="button"
                            class=move || if state.news_layout.get() != "cards" { "is-active" } else { "" }
                            aria-pressed=move || (state.news_layout.get() != "cards").to_string()
                            aria-label="Split reading"
                            title="Split reading (v)"
                            on:click=move |_| state.save_pref("newsLayout", json!("split"))
                        >
                            <Icon name="splitPane"/>
                        </button>
                        <button
                            type="button"
                            class=move || if state.news_layout.get() == "cards" { "is-active" } else { "" }
                            aria-pressed=move || (state.news_layout.get() == "cards").to_string()
                            aria-label="Magazine cards"
                            title="Magazine cards (v)"
                            on:click=move |_| {
                                state.selected.set(None);
                                state.article.set(None);
                                state.focus_mode.set(false);
                                state.save_pref("newsLayout", json!("cards"));
                            }
                        >
                            <Icon name="grid"/>
                        </button>
                    </div>
                    <button
                        type="button"
                        class="icon-button"
                        aria-label="Update feeds"
                        title="Update feeds (r)"
                        disabled=move || state.refreshing.get()
                        on:click=move |_| state.refresh()
                    >
                        <Icon name="refresh"/>
                    </button>
                    {move || (state.view_sig.get() != View::Starred).then(|| view! {
                        <button
                            type="button"
                            class="secondary-button news-mark-all"
                            disabled=move || state.busy.get() || !has_unread.get()
                            on:click=move |_| state.mark_all()
                            title="Mark all read (Shift A)"
                        >
                            <Icon name="checks"/><span>"Mark all read"</span>
                        </button>
                    })}
                    {move || match state.view_sig.get() {
                        View::Feed(id) => state
                            .overview
                            .with(|o| o.as_ref().and_then(|o| o.feeds.iter().find(|f| f.id == id).cloned()))
                            .map(|feed| {
                                let label = format!("Actions for {}", feed.title);
                                view! {
                                    <ActionMenu label=label actions=move |_| state.feed_actions(feed.clone())>
                                        <Icon name="more"/>
                                    </ActionMenu>
                                }.into_any()
                            }),
                        View::Folder(id) => state
                            .overview
                            .with(|o| o.as_ref().and_then(|o| o.folders.iter().find(|f| f.id == id).cloned()))
                            .map(|folder| {
                                let label = format!("Actions for folder {}", folder.name);
                                view! {
                                    <ActionMenu label=label actions=move |_| state.folder_actions(folder.clone())>
                                        <Icon name="more"/>
                                    </ActionMenu>
                                }.into_any()
                            }),
                        _ => None,
                    }}
                </div>
            </div>

            {move || if is_selecting.get() {
                let n = state.checked.with(HashSet::len);
                view! {
                    <div class="news-bulk-bar" role="region" aria-label="Selected articles">
                        <strong>{format!("{n} selected")}</strong>
                        <div class="news-bulk-actions">
                            <button
                                type="button"
                                class="icon-button"
                                title="Mark selected read"
                                aria-label="Mark selected read"
                                on:click=move |_| {
                                    state.set_state(checked_targets(), Some(false), None);
                                    state.checked.set(HashSet::new());
                                }
                            >
                                <Icon name="checks"/>
                            </button>
                            <button
                                type="button"
                                class="icon-button"
                                title="Mark selected unread"
                                aria-label="Mark selected unread"
                                on:click=move |_| {
                                    state.set_state(checked_targets(), Some(true), None);
                                    state.checked.set(HashSet::new());
                                }
                            >
                                <Icon name="unread"/>
                            </button>
                            <button
                                type="button"
                                class="icon-button"
                                title="Star selected"
                                aria-label="Star selected"
                                on:click=move |_| {
                                    state.set_state(checked_targets(), None, Some(true));
                                    state.checked.set(HashSet::new());
                                }
                            >
                                <Icon name="star"/>
                            </button>
                            <button
                                type="button"
                                class="icon-button"
                                title="Clear selection"
                                aria-label="Clear selection"
                                on:click=move |_| state.checked.set(HashSet::new())
                            >
                                <Icon name="close"/>
                            </button>
                        </div>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="news-filter-bar">
                        {filters.into_iter().map(|(target, label, icon)| {
                            let active = move || state.filter.get() == target;
                            view! {
                                <button
                                    type="button"
                                    class=move || if active() { "news-filter-chip is-active" } else { "news-filter-chip" }
                                    aria-pressed=move || active().to_string()
                                    on:click=move |_| state.filter.set(target)
                                >
                                    {icon.map(|ic| view! { <Icon name=ic/> })}
                                    <span>{label}</span>
                                </button>
                            }
                        }).collect_view()}
                        <button
                            type="button"
                            class="news-sort-btn"
                            title="Toggle sort order"
                            on:click=move |_| {
                                let next = if state.news_order.get_untracked() == "oldest" {
                                    "newest"
                                } else {
                                    "oldest"
                                };
                                state.save_pref("newsOrder", json!(next));
                            }
                        >
                            <Icon name="sort"/>
                            <span>{move || if state.news_order.get() == "oldest" { "Oldest" } else { "Newest" }}</span>
                        </button>
                    </div>
                }.into_any()
            }}

            {move || (!state.load_error.with(String::is_empty)).then(|| view! {
                <div class="news-banner is-error" role="alert">
                    <Icon name="alert"/>
                    <span><b>"News could not be loaded"</b>{state.load_error.get()}</span>
                    <button type="button" class="secondary-button" on:click=move |_| { state.load_overview(); state.load_items(); }>"Retry"</button>
                </div>
            })}

            {move || failing.get().into_iter().map(|feed| {
                let feed_id = feed.id;
                let heading = format!(
                    "{}: last {} update{} failed",
                    feed.title,
                    feed.update_error_count,
                    if feed.update_error_count == 1 { "" } else { "s" }
                );
                let detail = format!("{} Dar retries on its own; existing articles stay readable.", feed.last_update_error);
                view! {
                    <div class="news-banner" role="status">
                        <Icon name="alert"/>
                        <span><b>{heading}</b>{detail}</span>
                        <button
                            type="button"
                            class="secondary-button"
                            on:click=move |_| state.update_feed(feed_id, json!({ "updateNow": true }), Some("Feed updated"))
                        >
                            "Retry now"
                        </button>
                    </div>
                }
            }).collect_view()}

            {move || {
                let View::Feed(fid) = state.view_sig.get() else { return None };
                let info = state.feed_info.get().filter(|i| i.id == fid)?;
                let posts_14d: usize = info.per_day.iter().sum();
                let mut meta_parts = vec![
                    format!("{posts_14d} post{} / 14d", if posts_14d == 1 { "" } else { "s" }),
                ];
                if info.starred > 0 {
                    meta_parts.push(format!("{} starred", info.starred));
                }
                if info.full_text {
                    meta_parts.push("Full text on".into());
                }
                if info.paused {
                    meta_parts.push("Paused".into());
                }
                Some(view! {
                    <div class="news-feed-pulse">
                        <span class=mark_class(fid) aria-hidden="true">{initial(&info.title)}</span>
                        <div class="news-pulse-meta">
                            <strong>{info.title}</strong>
                            <small>{meta_parts.join(" \u{00b7} ")}</small>
                        </div>
                        <FeedSparkline per_day=info.per_day/>
                    </div>
                })
            }}

            <div
                class=move || format!(
                    "news-list{}{}",
                    if is_selecting.get() { " is-selecting" } else { "" },
                    if state.news_layout.get() == "cards" { " is-cards" } else { "" }
                )
                node_ref=state.list_ref
                role="list"
                aria-label=move || format!("{} articles", title.get())
            >
                {move || (state.list_state.get() == "loading" && state.items.with(Vec::is_empty)).then(|| view! {
                    <p class="news-empty" role="status">"Loading articles\u{2026}"</p>
                })}

                {move || (state.list_state.get() == "error").then(|| view! {
                    <div class="news-empty" role="alert">
                        <h2>"Articles could not be loaded"</h2>
                        <p>{state.list_error.get()}</p>
                        <button type="button" class="secondary-button" on:click=move |_| state.load_items()>"Retry"</button>
                    </div>
                })}

                {move || (state.list_state.get() == "ready" && visible.with(Vec::is_empty)).then(|| {
                    let no_feeds = state.overview.with(|o| o.as_ref().map(|o| o.feeds.is_empty()).unwrap_or(true));
                    let current_view = state.view_sig.get();
                    let active_filter = state.filter.get();
                    view! {
                        <div class="news-empty">
                            {if no_feeds {
                                view! {
                                    <h2>"No feeds yet"</h2>
                                    <p>"Subscribe to a site or import an OPML file from another reader."</p>
                                    <button type="button" class="primary-button" on:click=move |_| state.pane.set(Some(Pane::Subscribe))>"Subscribe"</button>
                                }.into_any()
                            } else if active_filter != ListFilter::All {
                                view! {
                                    <h2>"No matching articles"</h2>
                                    <p>"Nothing in this view matches the active filter."</p>
                                    <button type="button" class="secondary-button" on:click=move |_| state.filter.set(ListFilter::All)>"Show all"</button>
                                }.into_any()
                            } else if current_view == View::Starred {
                                view! {
                                    <h2>"No starred articles"</h2>
                                    <p>"Star an article with "<kbd>"s"</kbd>" to keep it here."</p>
                                }.into_any()
                            } else if current_view == View::Media {
                                view! {
                                    <h2>"No podcasts or media yet"</h2>
                                    <p>"Feeds with audio or video enclosures appear here automatically."</p>
                                }.into_any()
                            } else {
                                view! {
                                    <h2>"All caught up"</h2>
                                    <p>{match current_view {
                                        View::Unread => "There are no unread articles.",
                                        View::Today => "Nothing new has arrived today.",
                                        _ => "No articles in this view.",
                                    }}</p>
                                }.into_any()
                            }}
                        </div>
                    }
                })}

                {move || {
                    let items = visible.get();
                    if items.is_empty() {
                        return Vec::<AnyView>::new().into_any();
                    }
                    let layout = state.news_layout.get();
                    let show_images = state.news_images.get() != "block";
                    let feeds: HashMap<i64, String> = state.overview.with(|o| {
                        o.as_ref()
                            .map(|o| o.feeds.iter().map(|f| (f.id, f.title.clone())).collect())
                            .unwrap_or_default()
                    });

                    if layout == "cards" {
                        return view! {
                            <div class="news-cards-grid">
                                {items.into_iter().map(|item| render_card(state, item, &feeds, show_images)).collect_view()}
                            </div>
                        }.into_any();
                    }

                    let mut last_day = String::new();
                    items.into_iter().map(|item| {
                        let day = day_label(item.pub_date);
                        let heading = if day != last_day {
                            last_day = day.clone();
                            Some(view! { <div class="news-day" role="presentation">{day}</div> })
                        } else {
                            None
                        };
                        let row = render_row(state, item, &feeds, show_images);
                        view! { {heading}{row} }
                    }).collect_view().into_any()
                }}

                {move || (state.next_offset.get().is_some() && state.list_state.get() == "ready").then(|| view! {
                    <div class="news-more">
                        <button type="button" class="text-button" on:click=move |_| state.load_more()>"Load more"</button>
                    </div>
                })}
            </div>
        </main>
    }
}

fn render_row(
    state: NewsState,
    item: NewsItem,
    feeds: &HashMap<i64, String>,
    show_images: bool,
) -> impl IntoView {
    let item_id = item.id;
    let feed_id = item.feed_id;
    let unread = item.unread;
    let starred = item.starred;
    let item_url = item.url.clone();
    let feed_title = feeds
        .get(&feed_id)
        .cloned()
        .unwrap_or_else(|| "Feed".into());
    let mark_initial = initial(&feed_title);
    let item_title = if item.title.is_empty() {
        "Untitled".to_owned()
    } else {
        item.title.clone()
    };
    let thumb = if show_images {
        proxy_image_url(&item.media_thumbnail)
    } else {
        None
    };
    let has_media = !item.enclosure_link.is_empty();
    let is_video = is_video_enclosure(&item.enclosure_link, &item.enclosure_mime);
    let is_sel = move || state.selected.get() == Some(item_id);
    let is_chk = move || state.checked.with(|s| s.contains(&item_id));
    let has_thumb = thumb.is_some();

    view! {
        <div
            role="listitem"
            data-item=item_id
            class=move || format!(
                "news-article{}{}{}{}",
                if unread { "" } else { " is-read" },
                if is_sel() { " is-selected" } else { "" },
                if is_chk() { " is-checked" } else { "" },
                if has_thumb { " has-thumb" } else { "" }
            )
        >
            <input
                type="checkbox"
                class="news-row-check"
                aria-label=format!("Select {item_title}")
                prop:checked=is_chk
                on:change=move |e| {
                    let on = event_target_checked(&e);
                    state.checked.update(|set| {
                        if on {
                            set.insert(item_id);
                        } else {
                            set.remove(&item_id);
                        }
                    });
                }
            />
            <button
                type="button"
                class="news-article-open"
                aria-current=move || is_sel().then_some("true")
                on:click=move |_| state.open(item_id)
            >
                <span class="news-dot" aria-hidden="true"></span>
                <span class="news-article-content">
                    <span class="news-article-meta">
                        <span class=mark_class(feed_id) aria-hidden="true">{mark_initial}</span>
                        <span class="news-article-feed">{feed_title.clone()}</span>
                        {starred.then(|| view! {
                            <span class="news-star">
                                <Icon name="star"/>
                                <span class="sr-only">"Starred"</span>
                            </span>
                        })}
                        <time class="news-article-time" datetime=iso_string(item.pub_date)>
                            {time_label(item.pub_date)}
                        </time>
                    </span>
                    <span class="news-article-title">
                        {item_title}
                        {unread.then(|| view! { <span class="sr-only">", unread"</span> })}
                    </span>
                    {if has_media {
                        view! {
                            <span class="news-article-media-row">
                                <span class="news-media-pill">
                                    <Icon name=if is_video { "video" } else { "headphones" }/>
                                    {if is_video { "Video" } else { "Podcast" }}
                                </span>
                                {(!item.excerpt.is_empty()).then(|| view! {
                                    <span class="news-article-excerpt">{item.excerpt.clone()}</span>
                                })}
                            </span>
                        }.into_any()
                    } else {
                        (!item.excerpt.is_empty()).then(|| view! {
                            <span class="news-article-excerpt">{item.excerpt.clone()}</span>
                        }).into_any()
                    }}
                </span>
                {thumb.map(|src| view! {
                    <span class="news-thumb" aria-hidden="true">
                        <img class="news-thumb-img" src=src alt="" loading="lazy" decoding="async" referrerpolicy="no-referrer"/>
                    </span>
                })}
            </button>
            <div class="news-hover-triage">
                <button
                    type="button"
                    class=if starred { "icon-button is-starred" } else { "icon-button" }
                    aria-label=if starred { "Unstar article" } else { "Star article" }
                    title=if starred { "Unstar (s)" } else { "Star (s)" }
                    on:click=move |_| state.set_state(vec![(item_id, feed_id)], None, Some(!starred))
                >
                    <Icon name="star"/>
                </button>
                <button
                    type="button"
                    class="icon-button"
                    aria-label=if unread { "Mark read" } else { "Mark unread" }
                    title=if unread { "Mark read (m)" } else { "Mark unread (m)" }
                    on:click=move |_| state.set_state(vec![(item_id, feed_id)], Some(!unread), None)
                >
                    <Icon name=if unread { "check" } else { "unread" }/>
                </button>
                {(!item_url.is_empty()).then(|| view! {
                    <a
                        class="icon-button"
                        href=item_url
                        target="_blank"
                        rel="noopener noreferrer"
                        aria-label="Open original"
                        title="Open original (o)"
                    >
                        <Icon name="external"/>
                    </a>
                })}
            </div>
        </div>
    }
}

fn render_card(
    state: NewsState,
    item: NewsItem,
    feeds: &HashMap<i64, String>,
    show_images: bool,
) -> impl IntoView {
    let item_id = item.id;
    let feed_id = item.feed_id;
    let unread = item.unread;
    let starred = item.starred;
    let feed_title = feeds
        .get(&feed_id)
        .cloned()
        .unwrap_or_else(|| "Feed".into());
    let mark_initial = initial(&feed_title);
    let item_title = if item.title.is_empty() {
        "Untitled".to_owned()
    } else {
        item.title.clone()
    };
    let thumb = if show_images {
        proxy_image_url(&item.media_thumbnail)
    } else {
        None
    };
    let has_media = !item.enclosure_link.is_empty();
    let is_video = is_video_enclosure(&item.enclosure_link, &item.enclosure_mime);

    view! {
        <div
            role="listitem"
            data-item=item_id
            class=if unread { "news-card" } else { "news-card is-read" }
        >
            <button
                type="button"
                class="news-card-open"
                on:click=move |_| {
                    state.save_pref("newsLayout", json!("split"));
                    state.open(item_id);
                }
            >
                {match thumb {
                    Some(src) => view! {
                        <span class="news-card-media" aria-hidden="true">
                            <img class="news-thumb-img" src=src alt="" loading="lazy" decoding="async" referrerpolicy="no-referrer"/>
                        </span>
                    }.into_any(),
                    None => view! {
                        <span class=format!("news-card-media is-typo tone-{}", feed_id.rem_euclid(6)) aria-hidden="true">
                            <span class="news-card-typo-feed">{feed_title.clone()}</span>
                        </span>
                    }.into_any(),
                }}
                <span class="news-card-body">
                    <span class="news-article-meta">
                        {unread.then(|| view! { <span class="news-dot" aria-hidden="true"></span> })}
                        <span class=mark_class(feed_id) aria-hidden="true">{mark_initial}</span>
                        <span class="news-article-feed">{feed_title}</span>
                        <time class="news-article-time" datetime=iso_string(item.pub_date)>
                            {time_label(item.pub_date)}
                        </time>
                    </span>
                    <span class="news-card-title">{item_title}</span>
                    {(!item.excerpt.is_empty()).then(|| view! {
                        <span class="news-card-excerpt">{item.excerpt}</span>
                    })}
                    <span class="news-card-foot">
                        {has_media.then(|| view! {
                            <span class="news-media-pill">
                                <Icon name=if is_video { "video" } else { "headphones" }/>
                                {if is_video { "Video" } else { "Podcast" }}
                            </span>
                        })}
                        {starred.then(|| view! {
                            <span class="news-star">
                                <Icon name="star"/>
                                <span class="sr-only">"Starred"</span>
                            </span>
                        })}
                    </span>
                </span>
            </button>
        </div>
    }
}
