//! News side panes and confirmation dialog: Subscribe discovery, Edit feed (with 14-day cadence
//! sparkline), New/Rename folder, and destructive action confirmation.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use super::model::{
    BASE, ConfirmSpec, DiscoverResult, NewsDiscovered, NewsFeed, NewsFeedInfo, NewsFolder,
    day_label, relative,
};
use crate::api;
use crate::dates::local_date_time;
use crate::icons::Icon;
use crate::ui::{Dialog, EditorPane, Select};

/// Reusable 14-day posting cadence sparkline (pure SVG geometry — zero inline CSS styles for CSP).
#[component]
pub fn FeedSparkline(per_day: Vec<usize>) -> impl IntoView {
    let max = per_day.iter().copied().max().unwrap_or(0).max(1) as f64;
    let total: usize = per_day.iter().sum();
    let label = format!("{total} posts in the last 14 days");
    view! {
        <svg
            class="news-spark"
            viewBox="0 0 112 24"
            role="img"
            aria-label=label
        >
            {per_day.into_iter().enumerate().map(|(idx, count)| {
                let h = if count == 0 {
                    2.0
                } else {
                    (4.0 + ((count as f64) / max) * 18.0).round()
                };
                let y = 24.0 - h;
                let x = (idx * 8) as f64;
                let cls = if count == 0 { "news-spark-bar is-zero" } else { "news-spark-bar" };
                view! {
                    <rect class=cls x=x.to_string() y=y.to_string() width="5" height=h.to_string() rx="2"/>
                }
            }).collect_view()}
        </svg>
    }
}

#[component]
pub fn ConfirmAction(spec: ConfirmSpec, on_close: Callback<()>) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let title = spec.title;
    let text = spec.text;
    let action = StoredValue::new(spec.action);
    let run = StoredValue::new(spec.run);
    view! {
        <Dialog title=title on_close=on_close>
            <p>{text.clone()}</p>
            {move || (!error.with(String::is_empty)).then(|| view! {
                <p role="alert" class="error">{error.get()}</p>
            })}
            <div class="dialog-actions">
                <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                <button
                    type="button"
                    class="danger-button"
                    disabled=move || busy.get()
                    on:click=move |_| {
                        busy.set(true);
                        error.set(String::new());
                        let done = Callback::new(move |res: Result<(), String>| match res {
                            Ok(()) => { on_close.try_run(()); },
                            Err(msg) => {
                                error.set(msg);
                                busy.set(false);
                            }
                        });
                        run.with_value(|f| f(done));
                    }
                >
                    {move || if busy.get() { "Working\u{2026}".to_owned() } else { action.get_value() }}
                </button>
            </div>
        </Dialog>
    }
}

#[component]
pub fn SubscribePane(
    folders: Vec<NewsFolder>,
    csrf: String,
    on_close: Callback<()>,
    on_done: Callback<NewsFeed>,
    on_settings: Callback<()>,
) -> impl IntoView {
    let address = RwSignal::new(String::new());
    let found = RwSignal::new(None::<Vec<NewsDiscovered>>);
    let choice = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let folder = RwSignal::new("0".to_owned());
    let full_text = RwSignal::new(false);
    let busy = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let csrf = StoredValue::new(csrf);

    let mut folder_options = vec![("0".to_owned(), "No folder".to_owned())];
    for f in folders {
        folder_options.push((f.id.to_string(), f.name));
    }
    let folder_options = StoredValue::new(folder_options);

    let discover = move || {
        let addr = address.get_untracked().trim().to_owned();
        if addr.is_empty() {
            return;
        }
        busy.set("discover".into());
        error.set(String::new());
        found.set(None);
        let csrf = csrf.get_value();
        spawn_local(async move {
            match api::send::<DiscoverResult>(
                &format!("{BASE}/discover"),
                "POST",
                Some(json!({ "url": addr })),
                &csrf,
            )
            .await
            {
                Ok(result) => {
                    if let Some(first) = result.items.first() {
                        choice.set(first.url.clone());
                        name.set(first.title.clone());
                    }
                    found.set(Some(result.items));
                }
                Err(e) => error.set(e.message),
            }
            busy.set(String::new());
        });
    };

    let subscribe = move || {
        let selected_url = choice.get_untracked();
        if selected_url.is_empty() {
            return;
        }
        busy.set("subscribe".into());
        error.set(String::new());
        let folder_id = folder.get_untracked().parse::<i64>().unwrap_or(0);
        let trimmed_name = name.get_untracked().trim().to_owned();
        let ft = full_text.get_untracked();
        let mut body = serde_json::Map::new();
        body.insert("url".into(), json!(selected_url));
        body.insert("folderId".into(), json!(folder_id));
        if !trimmed_name.is_empty() {
            body.insert("title".into(), json!(trimmed_name));
        }
        body.insert("fullText".into(), json!(ft));
        let csrf = csrf.get_value();
        spawn_local(async move {
            match api::send::<NewsFeed>(
                &format!("{BASE}/feeds"),
                "POST",
                Some(Value::Object(body)),
                &csrf,
            )
            .await
            {
                Ok(feed) => {
                    on_done.try_run(feed);
                }
                Err(e) => {
                    error.set(e.message);
                    busy.set(String::new());
                }
            }
        });
    };

    view! {
        <EditorPane title="Subscribe" on_close=on_close>
            <form
                class="news-subscribe"
                on:submit=move |e: web_sys::SubmitEvent| {
                    e.prevent_default();
                    if found.with_untracked(Option::is_some) {
                        subscribe();
                    } else {
                        discover();
                    }
                }
            >
                <label class="news-field">
                    <span>"Site or feed address"</span>
                    <span class="news-address">
                        <input
                            autofocus
                            type="text"
                            inputmode="url"
                            required
                            prop:value=move || address.get()
                            placeholder="example.com or https://example.com/feed"
                            on:input=move |e| {
                                address.set(event_target_value(&e));
                                found.set(None);
                            }
                        />
                        <button
                            type="button"
                            class="secondary-button"
                            disabled=move || !busy.with(String::is_empty) || address.with(|a| a.trim().is_empty())
                            on:click=move |_| discover()
                        >
                            {move || if busy.get() == "discover" { "Looking\u{2026}" } else { "Find feeds" }}
                        </button>
                    </span>
                </label>

                {move || found.get().map(|list| {
                    let legend = if list.len() == 1 {
                        "Found 1 feed".to_owned()
                    } else {
                        format!("Found {} feeds", list.len())
                    };
                    let list_for_preview = list.clone();
                    view! {
                        <fieldset class="news-found">
                            <legend>{legend}</legend>
                            <div class="news-found-list">
                                {list.into_iter().map(|f| {
                                    let url_cls = f.url.clone();
                                    let url_chk = f.url.clone();
                                    let url_pick = f.url.clone();
                                    let title_pick = f.title.clone();
                                    view! {
                                        <label class=move || if choice.get() == url_cls { "news-found-option is-checked" } else { "news-found-option" }>
                                            <input
                                                type="radio"
                                                name="feed"
                                                prop:checked=move || choice.get() == url_chk
                                                on:change=move |_| {
                                                    choice.set(url_pick.clone());
                                                    name.set(title_pick.clone());
                                                }
                                            />
                                            <span class="news-found-meta">
                                                <b>{f.title}</b>
                                                <small>{format!("{} \u{00b7} {}", f.kind, f.url)}</small>
                                            </span>
                                        </label>
                                    }
                                }).collect_view()}
                            </div>
                        </fieldset>

                        <div class="news-form-row">
                            <label class="news-field">
                                <span>"Folder"</span>
                                <Select options=folder_options.get_value() value=folder/>
                            </label>
                            <label class="news-field">
                                <span>"Name"</span>
                                <input
                                    prop:value=move || name.get()
                                    maxlength="1000"
                                    on:input=move |e| name.set(event_target_value(&e))
                                />
                            </label>
                        </div>

                        <label class="news-check">
                            <input
                                type="checkbox"
                                prop:checked=move || full_text.get()
                                on:change=move |e| full_text.set(event_target_checked(&e))
                            />
                            <span>
                                <b>"Full text"</b>
                                <small>"Fetch each article page for the complete text"</small>
                            </span>
                        </label>

                        {move || {
                            let sel = choice.get();
                            list_for_preview
                                .iter()
                                .find(|f| f.url == sel)
                                .filter(|f| !f.items.is_empty())
                                .map(|f| view! {
                                    <div class="news-preview">
                                        <h3>"Latest items"</h3>
                                        <ul>
                                            {f.items.iter().map(|i| view! {
                                                <li>
                                                    <span>{if i.title.is_empty() { "Untitled".to_owned() } else { i.title.clone() }}</span>
                                                    <small>{day_label(i.pub_date)}</small>
                                                </li>
                                            }).collect_view()}
                                        </ul>
                                    </div>
                                })
                        }}
                    }
                })}

                {move || (!error.with(String::is_empty)).then(|| view! {
                    <p role="alert" class="error">{error.get()}</p>
                })}

                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_settings.try_run(()); }>
                        "Import OPML\u{2026}"
                    </button>
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>
                        "Cancel"
                    </button>
                    <button
                        class="primary-button"
                        disabled=move || {
                            !busy.with(String::is_empty)
                                || if found.with(Option::is_some) {
                                    choice.with(String::is_empty)
                                } else {
                                    address.with(|a| a.trim().is_empty())
                                }
                        }
                    >
                        {move || if found.with(Option::is_some) {
                            if busy.get() == "subscribe" { "Subscribing\u{2026}" } else { "Subscribe" }
                        } else if !busy.with(String::is_empty) {
                            "Looking\u{2026}"
                        } else {
                            "Find feeds"
                        }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}

#[component]
pub fn FeedPane(
    feed: NewsFeed,
    folders: Vec<NewsFolder>,
    csrf: String,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> impl IntoView {
    let title = RwSignal::new(feed.title.clone());
    let folder_id = RwSignal::new(feed.folder_id.to_string());
    let full_text = RwSignal::new(feed.full_text);
    let pinned = RwSignal::new(feed.pinned);
    let paused = RwSignal::new(feed.paused);
    let info = RwSignal::new(None::<NewsFeedInfo>);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let csrf = StoredValue::new(csrf);
    let feed_id = feed.id;
    let feed_url = StoredValue::new(feed.url.clone());
    let feed_link = StoredValue::new(feed.link.clone());
    let last_updated = feed.last_updated;
    let update_error_count = feed.update_error_count;
    let last_update_error = StoredValue::new(feed.last_update_error.clone());

    spawn_local(async move {
        if let Ok(loaded) = api::get::<NewsFeedInfo>(&format!("{BASE}/feeds/{feed_id}")).await {
            let _ = info.try_set(Some(loaded));
        }
    });

    let mut folder_options = vec![("0".to_owned(), "No folder".to_owned())];
    for f in folders {
        folder_options.push((f.id.to_string(), f.name));
    }
    let folder_options = StoredValue::new(folder_options);

    let save = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        busy.set(true);
        error.set(String::new());
        let body = json!({
            "title": title.get_untracked(),
            "folderId": folder_id.get_untracked().parse::<i64>().unwrap_or(0),
            "fullText": full_text.get_untracked(),
            "pinned": pinned.get_untracked(),
            "paused": paused.get_untracked(),
        });
        let csrf = csrf.get_value();
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/feeds/{feed_id}"),
                "PATCH",
                Some(body),
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    on_saved.try_run(());
                }
                Err(e) => {
                    error.set(e.message);
                    busy.set(false);
                }
            }
        });
    };

    view! {
        <EditorPane title="Edit feed" on_close=on_close>
            <form class="news-subscribe" on:submit=save>
                {move || info.get().map(|fi| {
                    let posts_14d: usize = fi.per_day.iter().sum();
                    view! {
                        <div class="news-pane-pulse">
                            <div class="news-pulse-meta">
                                <strong>{format!("{posts_14d} post{} in 14 days", if posts_14d == 1 { "" } else { "s" })}</strong>
                                <small>{format!("{} unread \u{00b7} {} starred", fi.unread_count, fi.starred)}</small>
                            </div>
                            <FeedSparkline per_day=fi.per_day/>
                        </div>
                    }
                })}

                <label class="news-field">
                    <span>"Name"</span>
                    <input
                        name="title"
                        required
                        maxlength="1000"
                        prop:value=move || title.get()
                        on:input=move |e| title.set(event_target_value(&e))
                    />
                </label>

                <label class="news-field">
                    <span>"Folder"</span>
                    <Select name="folderId" options=folder_options.get_value() value=folder_id/>
                </label>

                <label class="news-check">
                    <input
                        type="checkbox"
                        name="fullText"
                        prop:checked=move || full_text.get()
                        on:change=move |e| full_text.set(event_target_checked(&e))
                    />
                    <span>
                        <b>"Full text"</b>
                        <small>"Fetch each new article's page for the complete text"</small>
                    </span>
                </label>

                <label class="news-check">
                    <input
                        type="checkbox"
                        name="pinned"
                        prop:checked=move || pinned.get()
                        on:change=move |e| pinned.set(event_target_checked(&e))
                    />
                    <span>
                        <b>"Pin to top"</b>
                        <small>"Listed in the Pinned section of the sidebar"</small>
                    </span>
                </label>

                <label class="news-check">
                    <input
                        type="checkbox"
                        name="paused"
                        prop:checked=move || paused.get()
                        on:change=move |e| paused.set(event_target_checked(&e))
                    />
                    <span>
                        <b>"Pause updates"</b>
                        <small>"Keep its articles without checking for new ones"</small>
                    </span>
                </label>

                <dl class="news-facts">
                    <dt>"Feed URL"</dt>
                    <dd>
                        <a href=feed_url.get_value() target="_blank" rel="noopener noreferrer">
                            {feed_url.get_value()}
                        </a>
                    </dd>
                    {(!feed_link.get_value().is_empty()).then(|| view! {
                        <dt>"Website"</dt>
                        <dd>
                            <a href=feed_link.get_value() target="_blank" rel="noopener noreferrer">
                                {feed_link.get_value()}
                                " "<Icon name="external"/>
                            </a>
                        </dd>
                    })}
                    <dt>"Last update"</dt>
                    <dd>
                        {if last_updated > 0.0 {
                            format!("{} ({})", relative(last_updated), local_date_time(last_updated, &[]))
                        } else {
                            "Not yet".to_owned()
                        }}
                    </dd>
                    {(update_error_count > 0).then(|| view! {
                        <dt>"Problem"</dt>
                        <dd class="error">
                            {format!(
                                "{} ({} failed update{})",
                                last_update_error.get_value(),
                                update_error_count,
                                if update_error_count == 1 { "" } else { "s" }
                            )}
                        </dd>
                    })}
                </dl>

                {move || (!error.with(String::is_empty)).then(|| view! {
                    <p role="alert" class="error">{error.get()}</p>
                })}

                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get()>
                        {move || if busy.get() { "Saving\u{2026}" } else { "Save" }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}

#[component]
pub fn FolderPane(
    folder: Option<NewsFolder>,
    csrf: String,
    on_close: Callback<()>,
    on_saved: Callback<NewsFolder>,
) -> impl IntoView {
    let is_rename = folder.is_some();
    let folder_id = folder.as_ref().map(|f| f.id);
    let name = RwSignal::new(folder.map(|f| f.name).unwrap_or_default());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let csrf = StoredValue::new(csrf);

    let save = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let trimmed = name.get_untracked().trim().to_owned();
        busy.set(true);
        error.set(String::new());
        let csrf = csrf.get_value();
        spawn_local(async move {
            let (url, method) = match folder_id {
                Some(id) => (format!("{BASE}/folders/{id}"), "PATCH"),
                None => (format!("{BASE}/folders"), "POST"),
            };
            match api::send::<NewsFolder>(&url, method, Some(json!({ "name": trimmed })), &csrf)
                .await
            {
                Ok(saved) => {
                    on_saved.try_run(saved);
                }
                Err(e) => {
                    error.set(e.message);
                    busy.set(false);
                }
            }
        });
    };

    view! {
        <EditorPane title=if is_rename { "Rename folder" } else { "New folder" } on_close=on_close>
            <form class="news-subscribe" on:submit=save>
                <label class="news-field">
                    <span>"Name"</span>
                    <input
                        name="name"
                        autofocus
                        required
                        maxlength="200"
                        prop:value=move || name.get()
                        on:input=move |e| name.set(event_target_value(&e))
                    />
                </label>
                {move || (!error.with(String::is_empty)).then(|| view! {
                    <p role="alert" class="error">{error.get()}</p>
                })}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get()>
                        {move || if busy.get() {
                            "Saving\u{2026}"
                        } else if is_rename {
                            "Rename"
                        } else {
                            "Create folder"
                        }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}
