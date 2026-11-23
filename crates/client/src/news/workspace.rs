//! News (`next/client/news/workspace.tsx`): feeds and folders, a day-grouped list,
//! the sanitized article reader, subscribe/feed/folder panes and News settings.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::render::render_article;
use crate::api::{self, BrowserSession};
use crate::dates::{days_ago, local_date, local_date_time, local_time};
use crate::icons::Icon;
use crate::settings::{Notice, SaveProblems, Saver, Segmented, SettingRow, SettingSwitch, SettingsSheet};
use crate::shared::context;
use crate::shell::WorkspaceHeader;
use crate::ui::{
    active_element, document, listen, unique_id, window, ActionMenu, Dialog, EditorPane, MenuAction, Select,
};

const BASE: &str = "/api/v1/news";
const NEWS_REFRESH_CHOICES: [i64; 6] = [15, 30, 60, 180, 720, 1440];
const NEWS_KEEP_CHOICES: [i64; 4] = [7, 30, 90, 365];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewsFolder {
    id: i64,
    name: String,
    opened: bool,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewsFeed {
    id: i64,
    url: String,
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    folder_id: i64,
    #[serde(default)]
    added: f64,
    #[serde(default)]
    ordering: i64,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    full_text: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    unread_count: i64,
    #[serde(default)]
    last_updated: f64,
    #[serde(default)]
    next_update: f64,
    #[serde(default)]
    update_error_count: i64,
    #[serde(default)]
    last_update_error: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewsItem {
    id: i64,
    feed_id: i64,
    #[serde(default)]
    guid_hash: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    pub_date: f64,
    #[serde(default)]
    excerpt: String,
    #[serde(default)]
    enclosure_mime: String,
    #[serde(default)]
    enclosure_link: String,
    #[serde(default)]
    media_thumbnail: String,
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    starred: bool,
    #[serde(default)]
    last_modified: f64,
    #[serde(default)]
    position: f64,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewsItemDetail {
    id: i64,
    feed_id: i64,
    #[serde(default)]
    guid_hash: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    pub_date: f64,
    #[serde(default)]
    excerpt: String,
    #[serde(default)]
    enclosure_mime: String,
    #[serde(default)]
    enclosure_link: String,
    #[serde(default)]
    media_thumbnail: String,
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    starred: bool,
    #[serde(default)]
    last_modified: f64,
    #[serde(default)]
    position: f64,
    #[serde(default)]
    guid: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    updated_date: f64,
    #[serde(default)]
    media_description: String,
    #[serde(default)]
    rtl: bool,
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewsOverview {
    folders: Vec<NewsFolder>,
    feeds: Vec<NewsFeed>,
    starred_count: i64,
    unread_count: i64,
    newest_item_id: i64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewsPage {
    items: Vec<NewsItem>,
    next_offset: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewsDiscoveredItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    pub_date: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewsDiscovered {
    url: String,
    title: String,
    kind: String,
    #[serde(default)]
    items: Vec<NewsDiscoveredItem>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct DiscoverResult {
    items: Vec<NewsDiscovered>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct MarkReadResult {
    changed: i64,
    #[serde(default)]
    ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct ImportResult {
    added: i64,
    skipped: i64,
    folders: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Unread,
    Starred,
    All,
    Feed(i64),
    Folder(i64),
}

impl View {
    fn kind(self) -> &'static str {
        match self {
            View::Unread => "unread",
            View::Starred => "starred",
            View::All => "all",
            View::Feed(_) => "feed",
            View::Folder(_) => "folder",
        }
    }

    fn id(self) -> Option<i64> {
        match self {
            View::Feed(id) | View::Folder(id) => Some(id),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Pane {
    Subscribe,
    Feed(NewsFeed),
    Folder(Option<NewsFolder>),
    Settings,
}

#[derive(Clone)]
struct ConfirmSpec {
    title: String,
    text: String,
    action: String,
    run: Arc<dyn Fn(Callback<Result<(), String>>) + Send + Sync>,
}

#[derive(Clone, Copy)]
struct StateChange {
    id: i64,
    feed_id: i64,
    unread: Option<bool>,
    starred: Option<bool>,
}

fn mark_class(id: i64) -> String {
    format!("news-mark tone-{}", id.rem_euclid(6))
}

fn initial(title: &str) -> String {
    title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "\u{2022}".into())
}

fn minutes(value: i64) -> String {
    if value >= 60 {
        format!("{} hour{}", value / 60, if value == 60 { "" } else { "s" })
    } else {
        format!("{value} minutes")
    }
}

fn day_label(time: f64) -> String {
    let diff = days_ago(time);
    if diff <= 0 {
        "Today".into()
    } else if diff == 1 {
        "Yesterday".into()
    } else if diff < 7 {
        local_date(time, &[("weekday", "long")])
    } else {
        let same_year = js_sys::Date::new(&JsValue::from_f64(time)).get_full_year()
            == js_sys::Date::new_0().get_full_year();
        if same_year {
            local_date(time, &[("day", "numeric"), ("month", "long")])
        } else {
            local_date(time, &[("day", "numeric"), ("month", "long"), ("year", "numeric")])
        }
    }
}

fn time_label(time: f64) -> String {
    if day_label(time) == "Today" {
        local_time(time, &[("hour", "numeric"), ("minute", "2-digit")])
    } else {
        local_date(time, &[("month", "short"), ("day", "numeric")])
    }
}

fn relative(time: f64) -> String {
    let mins = ((js_sys::Date::now() - time) / 60_000.0).round() as i64;
    if mins < 1 {
        "just now".into()
    } else if mins < 60 {
        format!("{mins} min ago")
    } else if mins < 1440 {
        format!("{} h ago", ((mins as f64) / 60.0).round() as i64)
    } else {
        local_date(time, &[])
    }
}

fn iso_string(time: f64) -> String {
    if time.is_finite() {
        js_sys::Date::new(&JsValue::from_f64(time))
            .to_iso_string()
            .as_string()
            .unwrap_or_default()
    } else {
        String::new()
    }
}

fn is_playable_enclosure(link: &str, mime: &str) -> bool {
    if !link.starts_with("https:") {
        return false;
    }
    if mime.starts_with("audio/") || mime.starts_with("video/") {
        return true;
    }
    let path = link.split('?').next().unwrap_or(link).to_lowercase();
    [".mp3", ".m4a", ".ogg", ".opus", ".mp4", ".webm"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

fn copy_text(text: &str) {
    let nav = window().navigator();
    if let Ok(clip) = js_sys::Reflect::get(&nav, &JsValue::from_str("clipboard"))
        && !clip.is_undefined()
        && !clip.is_null()
        && let Ok(write) = js_sys::Reflect::get(&clip, &JsValue::from_str("writeText"))
        && let Some(func) = write.dyn_ref::<js_sys::Function>()
        && let Ok(promise) = func.call1(&clip, &JsValue::from_str(text))
        && let Ok(promise) = promise.dyn_into::<js_sys::Promise>()
    {
        spawn_local(async move {
            let _ = JsFuture::from(promise).await;
        });
    }
}

fn has_web_share() -> bool {
    let nav = window().navigator();
    js_sys::Reflect::has(&nav, &JsValue::from_str("share")).unwrap_or(false)
}

fn web_share(title: &str, url: &str) {
    let nav = window().navigator();
    if let Ok(share) = js_sys::Reflect::get(&nav, &JsValue::from_str("share"))
        && let Some(func) = share.dyn_ref::<js_sys::Function>()
    {
        let data = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&data, &JsValue::from_str("title"), &JsValue::from_str(title));
        let _ = js_sys::Reflect::set(&data, &JsValue::from_str("url"), &JsValue::from_str(url));
        if let Ok(promise) = func.call1(&nav, &data)
            && let Ok(promise) = promise.dyn_into::<js_sys::Promise>()
        {
            spawn_local(async move {
                let _ = JsFuture::from(promise).await;
            });
        }
    }
}

fn read_view() -> (View, Option<i64>) {
    let search = window().location().search().unwrap_or_default();
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok();
    let num = |key: &str| -> i64 {
        params
            .as_ref()
            .and_then(|p| p.get(key))
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let view = if num("feed") != 0 {
        View::Feed(num("feed"))
    } else if num("folder") != 0 {
        View::Folder(num("folder"))
    } else {
        match params.as_ref().and_then(|p| p.get("view")).as_deref() {
            Some("starred") => View::Starred,
            Some("all") => View::All,
            _ => View::Unread,
        }
    };
    let item = match num("item") {
        0 => None,
        n => Some(n),
    };
    (view, item)
}

fn write_view(view: View, item: Option<i64>) {
    let Ok(params) = web_sys::UrlSearchParams::new() else { return };
    match view {
        View::Feed(id) => params.set("feed", &id.to_string()),
        View::Folder(id) => params.set("folder", &id.to_string()),
        View::Starred => params.set("view", "starred"),
        View::All => params.set("view", "all"),
        View::Unread => {}
    }
    if let Some(id) = item {
        params.set("item", &id.to_string());
    }
    let query = params.to_string().as_string().unwrap_or_default();
    let next = if query.is_empty() { "/news".to_owned() } else { format!("/news?{query}") };
    let loc = window().location();
    let current = format!("{}{}", loc.pathname().unwrap_or_default(), loc.search().unwrap_or_default());
    if next != current
        && let Ok(history) = window().history()
    {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&next));
    }
}

fn items_query(view: View, order: &str, show_read: bool, offset: i64) -> String {
    let params = web_sys::UrlSearchParams::new().expect("params");
    params.set("type", view.kind());
    params.set("batch", "50");
    params.set("offset", &offset.to_string());
    params.set("oldestFirst", if order == "oldest" { "1" } else { "0" });
    let get_read = match view {
        View::Unread => "0",
        View::Feed(_) | View::Folder(_) if !show_read => "0",
        _ => "1",
    };
    params.set("getRead", get_read);
    if let Some(id) = view.id() {
        params.set("id", &id.to_string());
    }
    format!("{BASE}/items?{}", params.to_string().as_string().unwrap_or_default())
}

#[derive(Clone, Copy)]
struct NewsState {
    overview: RwSignal<Option<NewsOverview>>,
    load_error: RwSignal<String>,
    view_sig: RwSignal<View>,
    items: RwSignal<Vec<NewsItem>>,
    next_offset: RwSignal<Option<i64>>,
    list_state: RwSignal<&'static str>,
    list_error: RwSignal<String>,
    selected: RwSignal<Option<i64>>,
    article: RwSignal<Option<NewsItemDetail>>,
    article_error: RwSignal<String>,
    pane: RwSignal<Option<Pane>>,
    confirm: RwSignal<Option<ConfirmSpec>>,
    notice: RwSignal<Option<Notice>>,
    busy: RwSignal<bool>,
    refreshing: RwSignal<bool>,
    navigation: RwSignal<bool>,
    list_ref: NodeRef<html::Div>,
    generation: RwSignal<u32>,
    pending_read: StoredValue<HashSet<i64>>,
    session: StoredValue<BrowserSession>,
    news_mark_read: Memo<String>,
    news_order: Memo<String>,
    news_show_read: Memo<bool>,
    news_images: Memo<String>,
}

impl NewsState {
    fn csrf(self) -> String {
        self.session.with_value(|s| s.csrf_token.clone())
    }

    fn fail(self, message: String) {
        let msg = if message.is_empty() {
            "The request could not be completed.".into()
        } else {
            message
        };
        let _ = self.notice.try_set(Some(Notice { message: msg, undo: None }));
    }

    async fn fetch_overview(self) {
        match api::get::<NewsOverview>(&format!("{BASE}/overview")).await {
            Ok(data) => {
                let _ = self.overview.try_set(Some(data));
                let _ = self.load_error.try_set(String::new());
            }
            Err(e) => {
                let _ = self.load_error.try_set(e.message);
            }
        }
    }

    fn load_overview(self) {
        spawn_local(async move {
            self.fetch_overview().await;
        });
    }

    async fn fetch_items(self) {
        let Some(prev) = self.generation.try_get_untracked() else { return };
        let ticket = prev + 1;
        let _ = self.generation.try_set(ticket);
        let _ = self.list_state.try_set("loading");
        let url = items_query(
            self.view_sig.get_untracked(),
            &self.news_order.get_untracked(),
            self.news_show_read.get_untracked(),
            0,
        );
        match api::get::<NewsPage>(&url).await {
            Ok(page) => {
                if self.generation.try_get_untracked() != Some(ticket) {
                    return;
                }
                let _ = self.items.try_set(page.items);
                let _ = self.next_offset.try_set(page.next_offset);
                let _ = self.list_state.try_set("ready");
                if let Some(el) = self.list_ref.get_untracked() {
                    el.set_scroll_top(0);
                }
            }
            Err(e) => {
                if self.generation.try_get_untracked() == Some(ticket) {
                    let _ = self.list_state.try_set("error");
                    let _ = self.list_error.try_set(e.message);
                }
            }
        }
    }

    fn load_items(self) {
        spawn_local(async move {
            self.fetch_items().await;
        });
    }

    fn load_more(self) {
        let Some(offset) = self.next_offset.get_untracked() else { return };
        if self.list_state.get_untracked() != "ready" {
            return;
        }
        let ticket = self.generation.get_untracked();
        let url = items_query(
            self.view_sig.get_untracked(),
            &self.news_order.get_untracked(),
            self.news_show_read.get_untracked(),
            offset,
        );
        spawn_local(async move {
            let Ok(page) = api::get::<NewsPage>(&url).await else { return };
            if self.generation.try_get_untracked() != Some(ticket) {
                return;
            }
            let _ = self.items.try_update(|current| {
                for item in page.items {
                    if !current.iter().any(|c| c.id == item.id) {
                        current.push(item);
                    }
                }
            });
            let _ = self.next_offset.try_set(page.next_offset);
        });
    }

    /// Local count changes keep the sidebar in step without refetching after every read.
    fn adjust(self, changes: &[StateChange]) {
        let mut unread_delta = 0i64;
        let mut starred_delta = 0i64;
        let mut per_feed: HashMap<i64, i64> = HashMap::new();
        let current_items = self.items.get_untracked();
        let current_article = self.article.get_untracked();
        for c in changes {
            let old = current_items
                .iter()
                .find(|i| i.id == c.id)
                .map(|i| (i.unread, i.starred))
                .or_else(|| {
                    current_article
                        .as_ref()
                        .filter(|a| a.id == c.id)
                        .map(|a| (a.unread, a.starred))
                });
            if let Some((old_unread, old_starred)) = old {
                if let Some(u) = c.unread
                    && u != old_unread
                {
                    let d = if u { 1 } else { -1 };
                    unread_delta += d;
                    *per_feed.entry(c.feed_id).or_default() += d;
                }
                if let Some(s) = c.starred
                    && s != old_starred
                {
                    starred_delta += if s { 1 } else { -1 };
                }
            }
        }
        let _ = self.items.try_update(|list| {
            for item in list.iter_mut() {
                if let Some(c) = changes.iter().find(|c| c.id == item.id) {
                    if let Some(u) = c.unread {
                        item.unread = u;
                    }
                    if let Some(s) = c.starred {
                        item.starred = s;
                    }
                }
            }
        });
        let _ = self.overview.try_update(|ov| {
            if let Some(ov) = ov {
                ov.unread_count = (ov.unread_count + unread_delta).max(0);
                ov.starred_count = (ov.starred_count + starred_delta).max(0);
                for f in &mut ov.feeds {
                    if let Some(&d) = per_feed.get(&f.id) {
                        f.unread_count = (f.unread_count + d).max(0);
                    }
                }
            }
        });
        let _ = self.article.try_update(|art| {
            if let Some(art) = art
                && let Some(c) = changes.iter().find(|c| c.id == art.id)
            {
                if let Some(u) = c.unread {
                    art.unread = u;
                }
                if let Some(s) = c.starred {
                    art.starred = s;
                }
            }
        });
    }

    fn set_state(self, targets: Vec<(i64, i64)>, unread: Option<bool>, starred: Option<bool>) {
        if targets.is_empty() {
            return;
        }
        let forward: Vec<StateChange> = targets
            .iter()
            .map(|&(id, feed_id)| StateChange { id, feed_id, unread, starred })
            .collect();
        self.adjust(&forward);
        let mut body = serde_json::Map::new();
        body.insert("ids".into(), json!(targets.iter().map(|(id, _)| *id).collect::<Vec<_>>()));
        if let Some(u) = unread {
            body.insert("unread".into(), json!(u));
        }
        if let Some(s) = starred {
            body.insert("starred".into(), json!(s));
        }
        let csrf = self.csrf();
        spawn_local(async move {
            if let Err(e) = api::send::<Value>(&format!("{BASE}/items/state"), "POST", Some(Value::Object(body)), &csrf).await {
                let rollback: Vec<StateChange> = targets
                    .iter()
                    .map(|&(id, feed_id)| StateChange {
                        id,
                        feed_id,
                        unread: unread.map(|u| !u),
                        starred: starred.map(|s| !s),
                    })
                    .collect();
                self.adjust(&rollback);
                self.fail(e.message);
            }
        });
    }

    fn open(self, id: i64) {
        self.selected.set(Some(id));
        self.article_error.set(String::new());
        if self.article.with_untracked(|a| a.as_ref().map(|a| a.id)) != Some(id) {
            self.article.set(None);
        }
        let csrf = self.csrf();
        spawn_local(async move {
            match api::get::<NewsItemDetail>(&format!("{BASE}/items/{id}")).await {
                Ok(detail) => {
                    if self.selected.try_get_untracked() != Some(Some(id)) {
                        return;
                    }
                    let should_mark = detail.unread && self.news_mark_read.get_untracked() == "open";
                    let feed_id = detail.feed_id;
                    let _ = self.article.try_set(Some(detail));
                    if should_mark {
                        self.adjust(&[StateChange { id, feed_id, unread: Some(false), starred: None }]);
                        if let Err(e) = api::send::<Value>(
                            &format!("{BASE}/items/state"),
                            "POST",
                            Some(json!({ "ids": [id], "unread": false })),
                            &csrf,
                        )
                        .await
                        {
                            self.fail(e.message);
                        }
                        let _ = self.article.try_update(|a| {
                            if let Some(a) = a
                                && a.id == id
                            {
                                a.unread = false;
                            }
                        });
                    }
                }
                Err(e) => {
                    if self.selected.try_get_untracked() == Some(Some(id)) {
                        let _ = self.article_error.try_set(e.message);
                    }
                }
            }
        });
    }

    fn step(self, delta: i32, unread_only: bool) {
        let list = self.items.get_untracked();
        let sel = self.selected.get_untracked();
        let index = sel.and_then(|s| list.iter().position(|i| i.id == s));
        let next = if delta > 0 {
            let start = index.map(|i| i + 1).unwrap_or(0);
            list.get(start..).unwrap_or(&[]).iter().find(|i| !unread_only || i.unread).cloned()
        } else {
            let end = index.unwrap_or(0);
            list.get(..end).unwrap_or(&[]).iter().rev().find(|i| !unread_only || i.unread).cloned()
        };
        if let Some(next) = next {
            let next_id = next.id;
            self.open(next_id);
            let list_ref = self.list_ref;
            let cb = Closure::once_into_js(move || {
                if let Some(el) = list_ref
                    .get_untracked()
                    .and_then(|l| l.query_selector(&format!("[data-item=\"{next_id}\"]")).ok().flatten())
                {
                    let opts = web_sys::ScrollIntoViewOptions::new();
                    opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
                    el.scroll_into_view_with_scroll_into_view_options(&opts);
                }
            });
            let _ = window().request_animation_frame(cb.unchecked_ref());
        } else if delta > 0 && self.next_offset.get_untracked().is_some() {
            self.load_more();
        }
    }

    fn mark_all(self) {
        let items_max = self.items.with_untracked(|list| list.iter().map(|i| i.id).max().unwrap_or(0));
        let overview_max = self.overview.with_untracked(|o| o.as_ref().map(|o| o.newest_item_id).unwrap_or(0));
        let newest = 0i64.max(items_max).max(overview_max);
        let view = self.view_sig.get_untracked();
        let mut body = serde_json::Map::new();
        body.insert("type".into(), json!(view.kind()));
        if let Some(id) = view.id() {
            body.insert("id".into(), json!(id));
        }
        body.insert("newestItemId".into(), json!(newest));
        self.busy.set(true);
        let csrf = self.csrf();
        spawn_local(async move {
            match api::send::<MarkReadResult>(&format!("{BASE}/read"), "POST", Some(Value::Object(body)), &csrf).await {
                Ok(result) => {
                    let _ = self.items.try_update(|list| {
                        for item in list.iter_mut() {
                            item.unread = false;
                        }
                    });
                    self.fetch_overview().await;
                    let undo = (!result.ids.is_empty()).then(|| {
                        let ids: Vec<i64> = result.ids.into_iter().take(1000).collect();
                        let csrf_undo = csrf.clone();
                        Arc::new(move || {
                            let ids = ids.clone();
                            let csrf_undo = csrf_undo.clone();
                            spawn_local(async move {
                                match api::send::<Value>(
                                    &format!("{BASE}/items/state"),
                                    "POST",
                                    Some(json!({ "ids": ids, "unread": true })),
                                    &csrf_undo,
                                )
                                .await
                                {
                                    Ok(_) => {
                                        self.fetch_overview().await;
                                        self.fetch_items().await;
                                    }
                                    Err(e) => self.fail(e.message),
                                }
                            });
                        }) as Arc<dyn Fn() + Send + Sync>
                    });
                    let _ = self.notice.try_set(Some(Notice {
                        message: format!(
                            "{} article{} marked read",
                            result.changed,
                            if result.changed == 1 { "" } else { "s" }
                        ),
                        undo,
                    }));
                }
                Err(e) => self.fail(e.message),
            }
            let _ = self.busy.try_set(false);
        });
    }

    fn refresh(self) {
        self.refreshing.set(true);
        let csrf = self.csrf();
        spawn_local(async move {
            match api::send::<NewsOverview>(&format!("{BASE}/refresh"), "POST", None, &csrf).await {
                Ok(data) => {
                    let _ = self.overview.try_set(Some(data));
                    self.fetch_items().await;
                }
                Err(e) => self.fail(e.message),
            }
            let _ = self.refreshing.try_set(false);
        });
    }

    fn update_feed(self, feed_id: i64, change: Value, message: Option<&'static str>) {
        let reload_items = change.get("updateNow").and_then(Value::as_bool) == Some(true)
            || change.get("paused").and_then(Value::as_bool) == Some(false);
        let csrf = self.csrf();
        spawn_local(async move {
            match api::send::<Value>(&format!("{BASE}/feeds/{feed_id}"), "PATCH", Some(change), &csrf).await {
                Ok(_) => {
                    self.fetch_overview().await;
                    if reload_items {
                        self.fetch_items().await;
                    }
                    if let Some(msg) = message {
                        let _ = self.notice.try_set(Some(Notice { message: msg.into(), undo: None }));
                    }
                }
                Err(e) => self.fail(e.message),
            }
        });
    }

    fn choose(self, next: View) {
        self.view_sig.set(next);
        self.selected.set(None);
        self.article.set(None);
        self.navigation.set(false);
    }

    fn feed_actions(self, feed: NewsFeed) -> Vec<MenuAction> {
        let feed_id = feed.id;
        let feed_title = feed.title.clone();
        let feed_edit = feed.clone();
        let full_text = feed.full_text;
        let pinned = feed.pinned;
        let paused = feed.paused;
        vec![
            MenuAction::new("Mark all read", "checks").run(move || {
                self.choose(View::Feed(feed_id));
                let newest = self.overview.with_untracked(|o| o.as_ref().map(|o| o.newest_item_id).unwrap_or(0));
                let csrf = self.csrf();
                spawn_local(async move {
                    match api::send::<Value>(
                        &format!("{BASE}/read"),
                        "POST",
                        Some(json!({ "type": "feed", "id": feed_id, "newestItemId": newest })),
                        &csrf,
                    )
                    .await
                    {
                        Ok(_) => {
                            self.fetch_overview().await;
                            self.fetch_items().await;
                        }
                        Err(e) => self.fail(e.message),
                    }
                });
            }),
            MenuAction::new("Edit feed", "pencil").run(move || {
                self.pane.set(Some(Pane::Feed(feed_edit.clone())));
            }),
            MenuAction::new(if full_text { "Full text: on" } else { "Full text: off" }, "file").run(move || {
                self.update_feed(
                    feed_id,
                    json!({ "fullText": !full_text }),
                    Some(if full_text {
                        "Full text off for new articles"
                    } else {
                        "Full text on for new articles"
                    }),
                );
            }),
            MenuAction::new(if pinned { "Unpin" } else { "Pin to top" }, "pin").run(move || {
                self.update_feed(feed_id, json!({ "pinned": !pinned }), None);
            }),
            MenuAction::new(if paused { "Resume updates" } else { "Pause updates" }, "pause").run(move || {
                self.update_feed(
                    feed_id,
                    json!({ "paused": !paused }),
                    Some(if paused { "Updates resumed" } else { "Updates paused" }),
                );
            }),
            MenuAction::new("Update now", "refresh").disabled(paused).run(move || {
                self.update_feed(feed_id, json!({ "updateNow": true }), Some("Feed updated"));
            }),
            MenuAction::new("Unsubscribe\u{2026}", "trash")
                .danger()
                .separator()
                .run(move || {
                    let title = format!("Unsubscribe from {feed_title}?");
                    self.confirm.set(Some(ConfirmSpec {
                        title,
                        text: "This removes the feed and all of its saved articles, including starred ones.".into(),
                        action: "Unsubscribe".into(),
                        run: Arc::new(move |done: Callback<Result<(), String>>| {
                            let csrf = self.csrf();
                            spawn_local(async move {
                                match api::send::<Value>(&format!("{BASE}/feeds/{feed_id}"), "DELETE", None, &csrf).await {
                                    Ok(_) => {
                                        if self.view_sig.try_get_untracked() == Some(View::Feed(feed_id)) {
                                            self.choose(View::Unread);
                                        }
                                        self.fetch_overview().await;
                                        self.fetch_items().await;
                                        { done.try_run(Ok(())); };
                                    }
                                    Err(e) => { done.try_run(Err(e.message)); },
                                }
                            });
                        }),
                    }));
                }),
        ]
    }

    fn folder_actions(self, folder: NewsFolder) -> Vec<MenuAction> {
        let folder_id = folder.id;
        let folder_name = folder.name.clone();
        let folder_edit = folder.clone();
        vec![
            MenuAction::new("Mark all read", "checks").run(move || {
                let newest = self.overview.with_untracked(|o| o.as_ref().map(|o| o.newest_item_id).unwrap_or(0));
                let csrf = self.csrf();
                spawn_local(async move {
                    match api::send::<Value>(
                        &format!("{BASE}/read"),
                        "POST",
                        Some(json!({ "type": "folder", "id": folder_id, "newestItemId": newest })),
                        &csrf,
                    )
                    .await
                    {
                        Ok(_) => {
                            self.fetch_overview().await;
                            self.fetch_items().await;
                        }
                        Err(e) => self.fail(e.message),
                    }
                });
            }),
            MenuAction::new("Rename folder", "pencil").run(move || {
                self.pane.set(Some(Pane::Folder(Some(folder_edit.clone()))));
            }),
            MenuAction::new("Delete folder\u{2026}", "trash")
                .danger()
                .separator()
                .run(move || {
                    let count = self.overview.with_untracked(|o| {
                        o.as_ref()
                            .map(|o| o.feeds.iter().filter(|f| f.folder_id == folder_id).count())
                            .unwrap_or(0)
                    });
                    let text = if count > 0 {
                        format!(
                            "This unsubscribes from its {count} feed{} and removes their saved articles.",
                            if count == 1 { "" } else { "s" }
                        )
                    } else {
                        "The folder is empty.".into()
                    };
                    self.confirm.set(Some(ConfirmSpec {
                        title: format!("Delete {folder_name}?"),
                        text,
                        action: "Delete folder".into(),
                        run: Arc::new(move |done: Callback<Result<(), String>>| {
                            let csrf = self.csrf();
                            spawn_local(async move {
                                match api::send::<Value>(&format!("{BASE}/folders/{folder_id}"), "DELETE", None, &csrf).await {
                                    Ok(_) => {
                                        if self.view_sig.try_get_untracked() == Some(View::Folder(folder_id)) {
                                            self.choose(View::Unread);
                                        }
                                        self.fetch_overview().await;
                                        self.fetch_items().await;
                                        { done.try_run(Ok(())); };
                                    }
                                    Err(e) => { done.try_run(Err(e.message)); },
                                }
                            });
                        }),
                    }));
                }),
        ]
    }
}

/// News (selected mock A): feeds and folders, a day-grouped list and the article in the right sheet.
#[component]
pub fn NewsWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let (start_view, start_item) = read_view();
    let shared_prefs = context().preferences;
    let news_mark_read = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("newsMarkRead"))
                .and_then(Value::as_str)
                .unwrap_or("open")
                .to_owned()
        })
    });
    let news_order = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("newsOrder"))
                .and_then(Value::as_str)
                .unwrap_or("newest")
                .to_owned()
        })
    });
    let news_show_read = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("newsShowRead"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
    });
    let news_images = Memo::new(move |_| {
        shared_prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("newsImages"))
                .and_then(Value::as_str)
                .unwrap_or("proxy")
                .to_owned()
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
        selected: RwSignal::new(start_item),
        article: RwSignal::new(None),
        article_error: RwSignal::new(String::new()),
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
    };

    // Mobile navigation focus & Escape handling (`useMobileNavigation`).
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
            let body_active = document()
                .body()
                .is_some_and(|b| active.as_ref().is_some_and(|a| a.unchecked_ref::<web_sys::Node>() == b.as_ref()));
            let inside_nav = navigation_ref
                .get_untracked()
                .is_some_and(|r| r.contains(active.as_ref().map(|a| a.unchecked_ref::<web_sys::Node>())));
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

    // Overview initial load and 60-second background poll.
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }
    spawn_local(async move {
        state.fetch_overview().await;
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
        if state.news_mark_read.get() != "scroll" {
            return;
        }
        let Some(root) = state.list_ref.get() else { return };
        let init = web_sys::IntersectionObserverInit::new();
        let root_el: &web_sys::Element = &root;
        init.set_root(Some(root_el));
        init.set_threshold(&JsValue::from_f64(0.0));
        let callback = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
            let mut added = false;
            for idx in 0..entries.length() {
                let Ok(entry) = entries.get(idx).dyn_into::<web_sys::IntersectionObserverEntry>() else {
                    continue;
                };
                if !entry.is_intersecting()
                    && let Some(bounds) = entry.root_bounds()
                    && entry.bounding_client_rect().bottom() <= bounds.top() + 1.0
                    && let Some(id) = entry.target().get_attribute("data-item").and_then(|v| v.parse::<i64>().ok())
                {
                    let is_unread = state
                        .items
                        .with_untracked(|list| list.iter().find(|i| i.id == id).is_some_and(|i| i.unread));
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
                    let ids: Vec<i64> = state.pending_read.try_update_value(|set| set.drain().collect()).unwrap_or_default();
                    let targets: Vec<(i64, i64)> = state.items.with_untracked(|list| {
                        ids.into_iter()
                            .filter_map(|id| list.iter().find(|i| i.id == id && i.unread).map(|i| (i.id, i.feed_id)))
                            .collect()
                    });
                    state.set_state(targets, Some(false), None);
                });
            }
        });
        let Ok(observer) = web_sys::IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &init)
        else {
            return;
        };
        if let Ok(rows) = root.query_selector_all("[data-item]") {
            for i in 0..rows.length() {
                if let Some(el) = rows.item(i).and_then(|n| n.dyn_into::<web_sys::Element>().ok()) {
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
        let Some(root) = state.list_ref.get() else { return };
        let Some(sentinel) = root.query_selector(".news-more").ok().flatten() else { return };
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
        let Ok(observer) = web_sys::IntersectionObserver::new_with_options(callback.as_ref().unchecked_ref(), &init)
        else {
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
                    let _ = window().open_with_url_and_target_and_features(&url, "_blank", "noopener,noreferrer");
                }
            }
            "r" => {
                event.prevent_default();
                state.refresh();
            }
            "A" if event.shift_key() => {
                event.prevent_default();
                state.mark_all();
            }
            "Escape" if sel.is_some() => {
                event.prevent_default();
                state.selected.set(None);
                state.article.set(None);
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
        View::Starred => "Starred".into(),
        View::All => "All articles".into(),
        View::Unread => "Unread".into(),
    });

    let count = Memo::new(move |_| match state.view_sig.get() {
        View::Unread => state.overview.with(|o| o.as_ref().map(|o| o.unread_count)),
        View::Starred => state.overview.with(|o| o.as_ref().map(|o| o.starred_count)),
        View::Feed(id) => state.overview.with(|o| {
            o.as_ref()
                .and_then(|o| o.feeds.iter().find(|f| f.id == id))
                .map(|f| f.unread_count)
        }),
        View::Folder(id) => Some(state.overview.with(|o| {
            o.as_ref()
                .map(|o| o.feeds.iter().filter(|f| f.folder_id == id).map(|f| f.unread_count).sum())
                .unwrap_or(0)
        })),
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

    let feed_row = move |feed: NewsFeed, nested: bool| {
        let feed_id = feed.id;
        let feed_title = feed.title.clone();
        let feed_menu = feed.clone();
        let paused = feed.paused;
        let err_count = feed.update_error_count;
        let last_err = feed.last_update_error.clone();
        let unread_count = feed.unread_count;
        let active = move || state.view_sig.get() == View::Feed(feed_id);
        let row_class = if nested { "news-row news-feed-row is-nested" } else { "news-row news-feed-row" };
        let menu_label = format!("Actions for {feed_title}");
        view! {
            <div class=row_class>
                <button
                    class=move || format!("nav-item{}{}", if active() { " active" } else { "" }, if paused { " is-paused" } else { "" })
                    aria-current=move || active().then_some("page")
                    on:click=move |_| state.choose(View::Feed(feed_id))
                >
                    <span class=mark_class(feed_id) aria-hidden="true">{initial(&feed_title)}</span>
                    <span class="news-row-title">{feed_title.clone()}</span>
                    {(err_count > 0).then(|| view! {
                        <span class="news-warning" title=last_err.clone()><Icon name="alert"/><span class="sr-only">"Updates failing"</span></span>
                    })}
                    {paused.then(|| view! {
                        <span class="news-paused" title="Updates paused"><Icon name="pause"/><span class="sr-only">"Paused"</span></span>
                    })}
                    {(unread_count > 0).then(|| view! {
                        <span class="news-count">{unread_count}<span class="sr-only">" unread"</span></span>
                    })}
                </button>
                <ActionMenu label=menu_label class="news-row-menu" actions=move |_| state.feed_actions(feed_menu.clone())>
                    <Icon name="more"/>
                </ActionMenu>
            </div>
        }
    };

    let nav_views: [(View, &'static str, &'static str); 3] = [
        (View::Unread, "Unread", "list"),
        (View::Starred, "Starred", "star"),
        (View::All, "All articles", "file"),
    ];

    view! {
        <div class=move || format!("app-shell pim-shell news-shell{}", if state.selected.get().is_some() { " has-pim-details" } else { "" })>
            <WorkspaceHeader
                app="news"
                session=session.clone()
                on_signed_out=on_signed_out
                on_settings=Callback::new(move |_| state.pane.set(Some(Pane::Settings)))
            />
            <div class="pim-workspace news-workspace">
                {move || state.navigation.get().then(|| view! {
                    <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| state.navigation.set(false)></button>
                })}
                <aside
                    id=navigation_id
                    node_ref=navigation_ref
                    class=move || if state.navigation.get() { "sidebar news-sidebar is-open" } else { "sidebar news-sidebar" }
                    aria-label="News navigation"
                >
                    <button type="button" class="new-button ui-new" on:click=move |_| state.pane.set(Some(Pane::Subscribe))>
                        <Icon name="plus"/>"Subscribe"
                    </button>
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
                    <div class="sidebar-heading news-heading">
                        <h2>"Folders"</h2>
                        <button type="button" class="icon-button" aria-label="New folder" title="New folder" on:click=move |_| state.pane.set(Some(Pane::Folder(None)))>
                            <Icon name="plus"/>
                        </button>
                    </div>
                    <nav aria-label="Feeds" class="news-feeds">
                        {move || {
                            let Some(ov) = state.overview.get() else { return Vec::<AnyView>::new().into_any() };
                            let folders_view = ov.folders.iter().map(|folder| {
                                let folder_id = folder.id;
                                let folder_name = folder.name.clone();
                                let opened = folder.opened;
                                let folder_menu = folder.clone();
                                let children: Vec<NewsFeed> = ov.feeds.iter().filter(|f| f.folder_id == folder_id).cloned().collect();
                                let unread: i64 = children.iter().map(|f| f.unread_count).sum();
                                let any_failing = children.iter().any(|f| f.update_error_count > 0);
                                let active = move || state.view_sig.get() == View::Folder(folder_id);
                                let toggle_label = if opened {
                                    format!("Collapse {folder_name}")
                                } else {
                                    format!("Expand {folder_name}")
                                };
                                let menu_label = format!("Actions for folder {folder_name}");
                                view! {
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
                                                    ).await {
                                                        state.fail(e.message);
                                                    }
                                                });
                                            }
                                        >
                                            <Icon name="chevron"/>
                                        </button>
                                        <button
                                            class=move || format!("nav-item{}", if active() { " active" } else { "" })
                                            aria-current=move || active().then_some("page")
                                            on:click=move |_| state.choose(View::Folder(folder_id))
                                        >
                                            <Icon name="folder"/>
                                            <span class="news-row-title">{folder_name.clone()}</span>
                                            {any_failing.then(|| view! {
                                                <span class="news-warning"><Icon name="alert"/><span class="sr-only">"A feed is failing"</span></span>
                                            })}
                                            {(unread > 0).then(|| view! { <span class="news-count">{unread}</span> })}
                                        </button>
                                        <ActionMenu label=menu_label class="news-row-menu" actions=move |_| state.folder_actions(folder_menu.clone())>
                                            <Icon name="more"/>
                                        </ActionMenu>
                                    </div>
                                    {opened.then(|| children.into_iter().map(|feed| feed_row(feed, true)).collect_view())}
                                }
                            }).collect_view();
                            let unfiled = ov.feeds.into_iter().filter(|f| f.folder_id == 0).map(|feed| feed_row(feed, false)).collect_view();
                            view! { {folders_view}{unfiled} }.into_any()
                        }}
                    </nav>
                    {move || state.overview.get().map(|ov| {
                        let updated = ov.feeds.iter().map(|f| f.last_updated).fold(0.0f64, f64::max);
                        view! {
                            <div class="sidebar-footer news-footer">
                                <button type="button" class="text-button" disabled=move || state.refreshing.get() on:click=move |_| state.refresh()>
                                    <Icon name="refresh"/>
                                    {move || if state.refreshing.get() {
                                        "Updating\u{2026}".to_owned()
                                    } else if updated > 0.0 {
                                        format!("Updated {}", relative(updated))
                                    } else {
                                        "Update feeds".to_owned()
                                    }}
                                </button>
                            </div>
                        }
                    })}
                    <button type="button" class="nav-item ui-app-settings-entry" on:click=move |_| state.pane.set(Some(Pane::Settings))>
                        <Icon name="settings"/>"News settings"
                    </button>
                </aside>
                <main class="pim-main news-main" tabindex="-1" aria-label=move || title.get()>
                    <div class="news-list-heading">
                        <h1>{move || title.get()}</h1>
                        {move || count.get().map(|n| view! {
                            <span class="news-heading-count">
                                {n}
                                <span class="sr-only">{if state.view_sig.get() == View::Starred { " starred" } else { " unread" }}</span>
                            </span>
                        })}
                        <div class="news-tools">
                            <button type="button" class="icon-button" aria-label="Update feeds" title="Update feeds (r)" disabled=move || state.refreshing.get() on:click=move |_| state.refresh()>
                                <Icon name="refresh"/>
                            </button>
                            {move || (state.view_sig.get() != View::Starred).then(|| view! {
                                <button
                                    type="button"
                                    class="secondary-button news-mark-all"
                                    disabled=move || state.busy.get() || state.items.with(|list| !list.iter().any(|i| i.unread))
                                    on:click=move |_| state.mark_all()
                                    title="Mark all read (Shift A)"
                                >
                                    <Icon name="checks"/>"Mark all read"
                                </button>
                            })}
                            {move || match state.view_sig.get() {
                                View::Feed(id) => state.overview.with(|o| o.as_ref().and_then(|o| o.feeds.iter().find(|f| f.id == id).cloned())).map(|feed| {
                                    let label = format!("Actions for {}", feed.title);
                                    view! {
                                        <ActionMenu label=label actions=move |_| state.feed_actions(feed.clone())><Icon name="more"/></ActionMenu>
                                    }.into_any()
                                }),
                                View::Folder(id) => state.overview.with(|o| o.as_ref().and_then(|o| o.folders.iter().find(|f| f.id == id).cloned())).map(|folder| {
                                    let label = format!("Actions for folder {}", folder.name);
                                    view! {
                                        <ActionMenu label=label actions=move |_| state.folder_actions(folder.clone())><Icon name="more"/></ActionMenu>
                                    }.into_any()
                                }),
                                _ => None,
                            }}
                        </div>
                    </div>
                    {move || (!state.load_error.with(String::is_empty)).then(|| view! {
                        <div class="news-banner is-error" role="alert">
                            <Icon name="alert"/>
                            <span><b>"News could not be loaded"</b>{state.load_error.get()}</span>
                            <button class="secondary-button" on:click=move |_| { state.load_overview(); state.load_items(); }>"Retry"</button>
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
                                <button class="secondary-button" on:click=move |_| state.update_feed(feed_id, json!({ "updateNow": true }), Some("Feed updated"))>"Retry now"</button>
                            </div>
                        }
                    }).collect_view()}
                    <div class="news-list" node_ref=state.list_ref role="list" aria-label=move || format!("{} articles", title.get())>
                        {move || (state.list_state.get() == "loading" && state.items.with(Vec::is_empty)).then(|| view! {
                            <p class="news-empty" role="status">"Loading articles\u{2026}"</p>
                        })}
                        {move || (state.list_state.get() == "error").then(|| view! {
                            <div class="news-empty" role="alert">
                                <h2>"Articles could not be loaded"</h2>
                                <p>{state.list_error.get()}</p>
                                <button class="secondary-button" on:click=move |_| state.load_items()>"Retry"</button>
                            </div>
                        })}
                        {move || (state.list_state.get() == "ready" && state.items.with(Vec::is_empty)).then(|| {
                            let no_feeds = state.overview.with(|o| o.as_ref().map(|o| o.feeds.is_empty()).unwrap_or(true));
                            let current_view = state.view_sig.get();
                            view! {
                                <div class="news-empty">
                                    {if no_feeds {
                                        view! {
                                            <h2>"No feeds yet"</h2>
                                            <p>"Subscribe to a site or import an OPML file from another reader."</p>
                                            <button class="primary-button" on:click=move |_| state.pane.set(Some(Pane::Subscribe))>"Subscribe"</button>
                                        }.into_any()
                                    } else if current_view == View::Starred {
                                        view! {
                                            <h2>"No starred articles"</h2>
                                            <p>"Star an article with "<kbd>"s"</kbd>" to keep it here."</p>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <h2>"All caught up"</h2>
                                            <p>{if current_view == View::Unread { "There are no unread articles." } else { "No articles in this view." }}</p>
                                        }.into_any()
                                    }}
                                </div>
                            }
                        })}
                        {move || {
                            let items = state.items.get();
                            let feeds: HashMap<i64, String> = state.overview.with(|o| {
                                o.as_ref()
                                    .map(|o| o.feeds.iter().map(|f| (f.id, f.title.clone())).collect())
                                    .unwrap_or_default()
                            });
                            let mut last_day = String::new();
                            items.into_iter().map(|item| {
                                let day = day_label(item.pub_date);
                                let heading = if day != last_day {
                                    last_day = day.clone();
                                    Some(view! { <div class="news-day" role="presentation">{day}</div> })
                                } else {
                                    None
                                };
                                let item_id = item.id;
                                let unread = item.unread;
                                let feed_title = feeds.get(&item.feed_id).cloned();
                                let mark_initial = initial(feed_title.as_deref().unwrap_or("?"));
                                let excerpt_line = match (feed_title.as_deref().unwrap_or("Feed"), item.excerpt.as_str()) {
                                    (f, "") => f.to_owned(),
                                    (f, ex) => format!("{f} \u{00b7} {ex}"),
                                };
                                let item_title = if item.title.is_empty() { "Untitled".to_owned() } else { item.title.clone() };
                                let is_sel = move || state.selected.get() == Some(item_id);
                                view! {
                                    {heading}
                                    <div
                                        role="listitem"
                                        data-item=item_id
                                        class=move || format!("news-article{}{}", if unread { "" } else { " is-read" }, if is_sel() { " is-selected" } else { "" })
                                    >
                                        <button type="button" aria-current=move || is_sel().then_some("true") on:click=move |_| state.open(item_id)>
                                            <span class="news-dot" aria-hidden="true"></span>
                                            <span class=mark_class(item.feed_id) aria-hidden="true">{mark_initial}</span>
                                            <span class="news-article-title">
                                                {item_title}
                                                {unread.then(|| view! { <span class="sr-only">", unread"</span> })}
                                            </span>
                                            <span class="news-article-time">
                                                {item.starred.then(|| view! {
                                                    <span class="news-star"><Icon name="star"/><span class="sr-only">"Starred"</span></span>
                                                })}
                                                {(!item.enclosure_link.is_empty()).then(|| view! { <Icon name="clock"/> })}
                                                <time datetime=iso_string(item.pub_date)>{time_label(item.pub_date)}</time>
                                            </span>
                                            <span class="news-article-excerpt">{excerpt_line}</span>
                                        </button>
                                    </div>
                                }
                            }).collect_view()
                        }}
                        {move || (state.next_offset.get().is_some() && state.list_state.get() == "ready").then(|| view! {
                            <div class="news-more">
                                <button class="text-button" on:click=move |_| state.load_more()>"Load more"</button>
                            </div>
                        })}
                    </div>
                </main>
                <Show when=move || state.selected.get().is_some()>
                    <aside class="pim-details news-details" tabindex="-1" aria-label="Article">
                        <Reader
                            item=reader_item
                            feed=reader_feed
                            error=Signal::from(state.article_error)
                            images=Signal::from(state.news_images)
                            csrf=state.csrf()
                            on_close=Callback::new(move |_| {
                                state.selected.set(None);
                                state.article.set(None);
                            })
                            on_state=Callback::new(move |(unread, starred)| {
                                if let Some(art) = state.article.get_untracked() {
                                    state.set_state(vec![(art.id, art.feed_id)], unread, starred);
                                }
                            })
                            on_next=Callback::new(move |_| state.step(1, true))
                            on_retry=Callback::new(move |_| {
                                if let Some(id) = state.selected.get_untracked() {
                                    state.open(id);
                                }
                            })
                        />
                    </aside>
                </Show>
            </div>
            {move || state.pane.get().map(|p| {
                let folders = state.overview.with_untracked(|o| o.as_ref().map(|o| o.folders.clone()).unwrap_or_default());
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
                        let current = state.overview.with_untracked(|o| {
                            o.as_ref().and_then(|o| o.feeds.iter().find(|f| f.id == feed.id).cloned())
                        }).unwrap_or(feed);
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
                                        let _ = state.notice.try_set(Some(Notice { message: "Feed saved".into(), undo: None }));
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
                            <button type="button" class="text-button" on:click=move |_| { state.notice.set(None); undo(); }>"Undo"</button>
                        })}
                    </div>
                })}
            </div>
        </div>
    }
}

#[component]
fn ConfirmAction(spec: ConfirmSpec, on_close: Callback<()>) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let title = spec.title;
    let text = spec.text;
    let action = StoredValue::new(spec.action);
    let run = StoredValue::new(spec.run);
    view! {
        <Dialog title=title on_close=on_close>
            <p>{text.clone()}</p>
            {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            <div class="dialog-actions">
                <button class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                <button
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
fn Reader(
    item: Signal<Option<NewsItemDetail>>,
    feed: Signal<Option<NewsFeed>>,
    error: Signal<String>,
    images: Signal<String>,
    csrf: String,
    on_close: Callback<()>,
    on_state: Callback<(Option<bool>, Option<bool>)>,
    on_next: Callback<()>,
    on_retry: Callback<()>,
) -> impl IntoView {
    let body_ref = NodeRef::<html::Div>::new();
    let video_ref = NodeRef::<html::Video>::new();
    let audio_ref = NodeRef::<html::Audio>::new();
    let saved = RwSignal::new(0.0f64);
    let words = RwSignal::new(0usize);
    let csrf = StoredValue::new(csrf);

    let has_error = Memo::new(move |_| !error.with(String::is_empty));
    let item_id = Memo::new(move |_| item.with(|i| i.as_ref().map(|i| i.id)));
    let render_spec = Memo::new(move |_| {
        item.with(|i| i.as_ref().map(|i| (i.id, i.body.clone(), i.excerpt.clone(), images.get())))
    });
    let enclosure = Memo::new(move |_| {
        item.with(|i| {
            i.as_ref()
                .map(|i| (i.enclosure_link.clone(), i.enclosure_mime.clone(), i.media_description.clone()))
        })
    });

    // Rebuild sanitized DOM fragment into `.news-body`.
    Effect::new(move |_| {
        let Some(el) = body_ref.get() else { return };
        let spec = render_spec.get();
        while let Some(first) = el.first_child() {
            let _ = el.remove_child(&first);
        }
        if let Some((_, html, excerpt, imgs)) = spec {
            let frag = render_article(&html, &imgs);
            let _ = el.append_child(&frag);
            let text = el.text_content().unwrap_or(excerpt);
            words.set(text.split_whitespace().count());
        } else {
            words.set(0);
        }
    });

    // On phones, move focus to the article heading when an article opens.
    Effect::new(move |_| {
        if item_id.get().is_none() {
            return;
        }
        let Some(body_el) = body_ref.get() else { return };
        if window()
            .match_media("(max-width:800px)")
            .ok()
            .flatten()
            .is_some_and(|m| m.matches())
            && let Some(h1) = body_el
                .parent_element()
                .and_then(|p| p.query_selector("h1").ok().flatten())
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = h1.focus();
        }
    });

    // Restore saved media playback position when switching articles.
    Effect::new(move |_| {
        let _ = item_id.get();
        let pos = item.with_untracked(|i| i.as_ref().map(|i| i.position).unwrap_or(0.0));
        saved.set(pos);
        if pos > 0.0 {
            if let Some(v) = video_ref.get() {
                v.set_current_time(pos);
            }
            if let Some(a) = audio_ref.get() {
                a.set_current_time(pos);
            }
        }
    });

    let remember = move || {
        let current_time = video_ref
            .get_untracked()
            .map(|v| v.current_time())
            .or_else(|| audio_ref.get_untracked().map(|a| a.current_time()));
        let Some(ct) = current_time else { return };
        let Some(id) = item_id.get_untracked() else { return };
        if (ct - saved.get_untracked()).abs() < 15.0 {
            return;
        }
        saved.set(ct);
        let csrf = csrf.get_value();
        spawn_local(async move {
            let _ = api::send::<Value>(
                &format!("{BASE}/items/{id}/position"),
                "POST",
                Some(json!({ "position": ct.floor() as i64 })),
                &csrf,
            )
            .await;
        });
    };

    let share_actions = move |_| {
        let Some((title, url)) = item.with_untracked(|i| i.as_ref().filter(|i| !i.url.is_empty()).map(|i| (i.title.clone(), i.url.clone()))) else {
            return Vec::new();
        };
        let copy_url = url.clone();
        let mut actions = vec![MenuAction::new("Copy link", "copy").run(move || copy_text(&copy_url))];
        if has_web_share() {
            actions.push(MenuAction::new("Share\u{2026}", "share").run(move || web_share(&title, &url)));
        }
        actions
    };

    view! {
        <section class="news-reader">
            <div class="news-reader-head">
                <button type="button" class="icon-button news-back" aria-label="Back to articles" on:click=move |_| { on_close.try_run(()); }>
                    <Icon name="back"/>
                </button>
                {move || feed.get().map(|f| view! {
                    <span class="news-reader-feed">
                        <span class=mark_class(f.id) aria-hidden="true">{initial(&f.title)}</span>
                        {f.title}
                    </span>
                })}
                <Show when=move || item_id.get().is_some()>
                    <div class="news-reader-tools">
                        <button
                            type="button"
                            class=move || if item.with(|i| i.as_ref().is_some_and(|i| i.starred)) { "icon-button is-starred" } else { "icon-button" }
                            aria-pressed=move || item.with(|i| i.as_ref().is_some_and(|i| i.starred)).to_string()
                            aria-label="Star"
                            title="Star (s)"
                            on:click=move |_| {
                                if let Some(s) = item.with_untracked(|i| i.as_ref().map(|i| i.starred)) {
                                    { on_state.try_run((None, Some(!s))); };
                                }
                            }
                        >
                            <Icon name="star"/>
                        </button>
                        <button
                            type="button"
                            class="icon-button"
                            aria-pressed=move || item.with(|i| i.as_ref().is_some_and(|i| i.unread)).to_string()
                            aria-label="Keep unread"
                            title="Keep unread (m)"
                            on:click=move |_| {
                                if let Some(u) = item.with_untracked(|i| i.as_ref().map(|i| i.unread)) {
                                    { on_state.try_run((Some(!u), None)); };
                                }
                            }
                        >
                            <Icon name="unread"/>
                        </button>
                        {move || item.with(|i| i.as_ref().filter(|i| !i.url.is_empty()).map(|i| i.url.clone())).map(|url| view! {
                            <a class="icon-button" href=url target="_blank" rel="noopener noreferrer" aria-label="Open original" title="Open original (o)">
                                <Icon name="external"/>
                            </a>
                        })}
                        {move || item.with(|i| i.as_ref().is_some_and(|i| !i.url.is_empty())).then(|| view! {
                            <ActionMenu label="Share" actions=share_actions><Icon name="share"/></ActionMenu>
                        })}
                        <button type="button" class="icon-button news-close" aria-label="Close article" on:click=move |_| { on_close.try_run(()); }>
                            <Icon name="close"/>
                        </button>
                    </div>
                </Show>
            </div>
            {move || if has_error.get() {
                view! {
                    <div class="news-empty" role="alert">
                        <h2>"This article could not be opened"</h2>
                        <p>{move || error.get()}</p>
                        <button class="secondary-button" on:click=move |_| { on_retry.try_run(()); }>"Retry"</button>
                    </div>
                }.into_any()
            } else if item_id.get().is_none() {
                view! { <p class="news-empty" role="status">"Opening article\u{2026}"</p> }.into_any()
            } else {
                view! {
                    <article class="news-article-body" dir=move || item.with(|i| i.as_ref().filter(|i| i.rtl).map(|_| "rtl"))>
                        {move || feed.with(|f| f.as_ref().is_some_and(|f| f.full_text)).then(|| view! {
                            <span class="news-fulltext"><Icon name="file"/>"Full text"</span>
                        })}
                        <h1 tabindex="-1">
                            {move || item.with(|i| i.as_ref().map(|i| {
                                let title = if i.title.is_empty() { "Untitled".to_owned() } else { i.title.clone() };
                                if i.url.is_empty() {
                                    view! { {title} }.into_any()
                                } else {
                                    view! { <a href=i.url.clone() target="_blank" rel="noopener noreferrer">{title}</a> }.into_any()
                                }
                            }))}
                        </h1>
                        <p class="news-byline">
                            {move || item.with(|i| i.as_ref().map(|i| {
                                let mut parts = Vec::new();
                                if !i.author.is_empty() {
                                    parts.push(format!("By {}", i.author));
                                }
                                parts.push(local_date_time(i.pub_date, &[("dateStyle", "medium"), ("timeStyle", "short")]));
                                let w = words.get();
                                if w > 120 {
                                    let mins = ((w as f64) / 230.0).round().max(1.0) as usize;
                                    parts.push(format!("{mins} min read"));
                                }
                                parts.join(" \u{00b7} ")
                            }).unwrap_or_default())}
                        </p>
                        {move || {
                            let Some((link, mime, desc)) = enclosure.get() else { return None };
                            if link.is_empty() {
                                return None;
                            }
                            if is_playable_enclosure(&link, &mime) {
                                let is_video = mime.starts_with("video/");
                                Some(view! {
                                    <div class="news-player">
                                        {if is_video {
                                            view! {
                                                <video node_ref=video_ref controls preload="metadata" src=link on:pause=move |_| remember() on:timeupdate=move |_| remember()/>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <audio node_ref=audio_ref controls preload="metadata" src=link on:pause=move |_| remember() on:timeupdate=move |_| remember()/>
                                            }.into_any()
                                        }}
                                        {(!desc.is_empty()).then(|| view! { <p>{desc}</p> })}
                                    </div>
                                }.into_any())
                            } else if !link.starts_with("https:") {
                                Some(view! {
                                    <p><a href=link target="_blank" rel="noopener noreferrer">"Open attachment"</a></p>
                                }.into_any())
                            } else {
                                None
                            }
                        }}
                        <div node_ref=body_ref class="news-body"></div>
                    </article>
                }.into_any()
            }}
            <Show when=move || item_id.get().is_some()>
                <div class="news-reader-foot">
                    <span><kbd>"j"</kbd>" "<kbd>"k"</kbd>" Next / previous"</span>
                    <button type="button" class="secondary-button" on:click=move |_| { on_next.try_run(()); }>
                        "Next unread"<Icon name="chevron"/>
                    </button>
                </div>
            </Show>
        </section>
    }
}

#[component]
fn SubscribePane(
    folders: Vec<NewsFolder>,
    csrf: String,
    on_close: Callback<()>,
    on_done: Callback<NewsFeed>,
    on_settings: Callback<()>,
) -> impl IntoView {
    let address = RwSignal::new(String::new());
    let found: RwSignal<Option<Vec<NewsDiscovered>>> = RwSignal::new(None);
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
        let trimmed = address.get_untracked().trim().to_owned();
        if trimmed.is_empty() {
            return;
        }
        busy.set("discover".into());
        error.set(String::new());
        found.set(None);
        let csrf = csrf.get_value();
        spawn_local(async move {
            match api::send::<DiscoverResult>(&format!("{BASE}/discover"), "POST", Some(json!({ "url": trimmed })), &csrf).await {
                Ok(result) => {
                    if let Some(first) = result.items.first() {
                        choice.set(first.url.clone());
                        name.set(first.title.clone());
                    } else {
                        choice.set(String::new());
                        name.set(String::new());
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
            match api::send::<NewsFeed>(&format!("{BASE}/feeds"), "POST", Some(Value::Object(body)), &csrf).await {
                Ok(feed) => { on_done.try_run(feed); },
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
                <label>
                    "Site or feed address"
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
                            {list.into_iter().map(|f| {
                                let url_cls = f.url.clone();
                                let url_chk = f.url.clone();
                                let url_pick = f.url.clone();
                                let title_pick = f.title.clone();
                                view! {
                                    <label class=move || if choice.get() == url_cls { "is-checked" } else { "" }>
                                        <input
                                            type="radio"
                                            name="feed"
                                            prop:checked=move || choice.get() == url_chk
                                            on:change=move |_| {
                                                choice.set(url_pick.clone());
                                                name.set(title_pick.clone());
                                            }
                                        />
                                        <span>
                                            <b>{f.title}</b>
                                            <small>{format!("{} \u{00b7} {}", f.kind, f.url)}</small>
                                        </span>
                                    </label>
                                }
                            }).collect_view()}
                        </fieldset>
                        <div class="news-form-row">
                            <label>"Folder"<Select options=folder_options.get_value() value=folder/></label>
                            <label>"Name"<input prop:value=move || name.get() maxlength="1000" on:input=move |e| name.set(event_target_value(&e))/></label>
                        </div>
                        <label class="news-check">
                            <input type="checkbox" prop:checked=move || full_text.get() on:change=move |e| full_text.set(event_target_checked(&e))/>
                            <span><b>"Full text"</b><small>"Fetch each article page for the complete text"</small></span>
                        </label>
                        {move || {
                            let sel = choice.get();
                            list_for_preview.iter().find(|f| f.url == sel).filter(|f| !f.items.is_empty()).map(|f| view! {
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
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_settings.try_run(()); }>"Import OPML\u{2026}"</button>
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
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
fn FeedPane(
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
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let csrf = StoredValue::new(csrf);
    let feed_id = feed.id;
    let feed_url = StoredValue::new(feed.url.clone());
    let last_updated = feed.last_updated;
    let update_error_count = feed.update_error_count;
    let last_update_error = StoredValue::new(feed.last_update_error.clone());

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
            match api::send::<Value>(&format!("{BASE}/feeds/{feed_id}"), "PATCH", Some(body), &csrf).await {
                Ok(_) => { on_saved.try_run(()); },
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
                <label>"Name"<input name="title" required maxlength="1000" prop:value=move || title.get() on:input=move |e| title.set(event_target_value(&e))/></label>
                <label>"Folder"<Select name="folderId" options=folder_options.get_value() value=folder_id/></label>
                <label class="news-check">
                    <input type="checkbox" name="fullText" prop:checked=move || full_text.get() on:change=move |e| full_text.set(event_target_checked(&e))/>
                    <span><b>"Full text"</b><small>"Fetch each new article's page for the complete text"</small></span>
                </label>
                <label class="news-check">
                    <input type="checkbox" name="pinned" prop:checked=move || pinned.get() on:change=move |e| pinned.set(event_target_checked(&e))/>
                    <span><b>"Pin to top"</b><small>"Listed first in the sidebar"</small></span>
                </label>
                <label class="news-check">
                    <input type="checkbox" name="paused" prop:checked=move || paused.get() on:change=move |e| paused.set(event_target_checked(&e))/>
                    <span><b>"Pause updates"</b><small>"Keep its articles without checking for new ones"</small></span>
                </label>
                <dl class="news-facts">
                    <dt>"Address"</dt>
                    <dd><a href=feed_url.get_value() target="_blank" rel="noopener noreferrer">{feed_url.get_value()}</a></dd>
                    <dt>"Last update"</dt>
                    <dd>{if last_updated > 0.0 { local_date_time(last_updated, &[]) } else { "Not yet".to_owned() }}</dd>
                    {(update_error_count > 0).then(|| view! {
                        <dt>"Problem"</dt>
                        <dd class="error">{format!("{} ({} failed update{})", last_update_error.get_value(), update_error_count, if update_error_count == 1 { "" } else { "s" })}</dd>
                    })}
                </dl>
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get()>{move || if busy.get() { "Saving\u{2026}" } else { "Save" }}</button>
                </div>
            </form>
        </EditorPane>
    }
}

#[component]
fn FolderPane(
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
            match api::send::<NewsFolder>(&url, method, Some(json!({ "name": trimmed })), &csrf).await {
                Ok(saved) => { on_saved.try_run(saved); },
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
                <label>"Name"<input name="name" autofocus required maxlength="200" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/></label>
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
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

#[component]
fn NewsSettings(
    session: BrowserSession,
    on_close: Callback<()>,
    on_imported: Callback<()>,
) -> impl IntoView {
    let username = StoredValue::new(session.user.username.clone());
    let csrf = StoredValue::new(session.csrf_token.clone());
    let saver = Saver::new(session);
    let section = RwSignal::new("General");
    let importing = RwSignal::new(false);
    let import_result = RwSignal::new(String::new());
    let import_error = RwSignal::new(String::new());

    let text = move |key: &'static str, default: &'static str| {
        Signal::derive(move || saver.value(key).as_str().unwrap_or(default).to_owned())
    };
    let flag = move |key: &'static str, default: bool| {
        Signal::derive(move || saver.value(key).as_bool().unwrap_or(default))
    };

    let refresh_val = RwSignal::new("30".to_owned());
    let keep_val = RwSignal::new("30".to_owned());
    Effect::new(move |_| {
        let r = saver.value("newsRefresh").as_i64().unwrap_or(30).to_string();
        if refresh_val.get_untracked() != r {
            refresh_val.set(r);
        }
        let k = saver.value("newsKeepDays").as_i64().unwrap_or(30).to_string();
        if keep_val.get_untracked() != k {
            keep_val.set(k);
        }
    });

    let refresh_options: StoredValue<Vec<(String, String)>> = StoredValue::new(
        NEWS_REFRESH_CHOICES
            .iter()
            .map(|&v| (v.to_string(), format!("Every {}", minutes(v))))
            .collect(),
    );
    let keep_options: StoredValue<Vec<(String, String)>> = StoredValue::new(
        NEWS_KEEP_CHOICES
            .iter()
            .map(|&v| (v.to_string(), format!("{v} days")))
            .collect(),
    );

    let address = StoredValue::new(format!(
        "{}/dav/news",
        window().location().origin().unwrap_or_default()
    ));

    let import_file = move |file: Option<web_sys::File>| {
        let Some(file) = file else { return };
        importing.set(true);
        import_error.set(String::new());
        import_result.set(String::new());
        let file = send_wrapper::SendWrapper::new(file);
        let csrf = csrf.get_value();
        spawn_local(async move {
            let file = file.take();
            if file.size() > 2.0 * 1024.0 * 1024.0 {
                import_error.set("Choose an OPML file up to 2 MiB.".into());
                importing.set(false);
                return;
            }
            let text = JsFuture::from(file.text())
                .await
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            match api::send::<ImportResult>(&format!("{BASE}/opml"), "POST", Some(json!({ "text": text })), &csrf).await {
                Ok(result) => {
                    let folders_part = if result.folders > 0 {
                        format!(" into {} new folder{}", result.folders, if result.folders == 1 { "" } else { "s" })
                    } else {
                        String::new()
                    };
                    let skipped_part = if result.skipped > 0 {
                        format!("; {} already subscribed", result.skipped)
                    } else {
                        String::new()
                    };
                    import_result.set(format!(
                        "Imported {} feed{}{}{}. New feeds fill in as they update.",
                        result.added,
                        if result.added == 1 { "" } else { "s" },
                        folders_part,
                        skipped_part
                    ));
                    { on_imported.try_run(()); };
                }
                Err(e) => import_error.set(e.message),
            }
            importing.set(false);
        });
    };

    view! {
        <SettingsSheet
            title="News settings"
            icon="rss"
            sections=vec!["General", "Import and export", "Keyboard shortcuts"]
            section=section
            on_close=on_close
            notice=saver.notice
        >
            <SaveProblems saver=saver/>
            {move || match section.get() {
                "General" if saver.preferences.with(Option::is_some) => {
                    let addr = address.get_value();
                    let addr_copy = addr.clone();
                    let user = username.get_value();
                    view! {
                        <section class="settings-group" aria-label="Reading">
                            <h3>"Reading"</h3>
                            <SettingRow
                                label="Mark as read"
                                hint="When an article opens, or as it scrolls past in the list"
                                state=saver.state("newsMarkRead")
                            >
                                <Segmented
                                    label="Mark as read"
                                    value=text("newsMarkRead", "open")
                                    options=vec![("open", "On open"), ("scroll", "On scroll")]
                                    on_change=Callback::new(move |value: String| {
                                        let msg = if value == "open" {
                                            "Articles are marked read when opened"
                                        } else {
                                            "Articles are marked read as they scroll past"
                                        };
                                        saver.change("newsMarkRead", json!(value), msg.into(), true);
                                    })
                                />
                            </SettingRow>
                            <SettingRow label="Order" hint="" state=saver.state("newsOrder")>
                                <Segmented
                                    label="Order"
                                    value=text("newsOrder", "newest")
                                    options=vec![("newest", "Newest first"), ("oldest", "Oldest first")]
                                    on_change=Callback::new(move |value: String| {
                                        let msg = format!("Showing {value} articles first");
                                        saver.change("newsOrder", json!(value), msg, true);
                                    })
                                />
                            </SettingRow>
                            <SettingSwitch
                                label="Show read articles in feeds and folders"
                                hint="Unread always shows only unread articles"
                                state=saver.state("newsShowRead")
                                checked=flag("newsShowRead", false)
                                on_change=Callback::new(move |value: bool| {
                                    saver.change(
                                        "newsShowRead",
                                        json!(value),
                                        if value { "Read articles shown" } else { "Read articles hidden" }.into(),
                                        true,
                                    );
                                })
                            />
                            <SettingRow
                                label="Remote images"
                                hint="Through Dar keeps sites from seeing when you read"
                                state=saver.state("newsImages")
                            >
                                <Segmented
                                    label="Remote images"
                                    value=text("newsImages", "proxy")
                                    options=vec![("proxy", "Through Dar"), ("block", "Block")]
                                    on_change=Callback::new(move |value: String| {
                                        let msg = if value == "proxy" {
                                            "Images load through Dar"
                                        } else {
                                            "Images blocked"
                                        };
                                        saver.change("newsImages", json!(value), msg.into(), true);
                                    })
                                />
                            </SettingRow>
                        </section>
                        <section class="settings-group" aria-label="Updates">
                            <h3>"Updates"</h3>
                            <SettingRow
                                label="Check feeds"
                                hint="Failing feeds back off automatically"
                                state=saver.state("newsRefresh")
                            >
                                <Select
                                    aria_label="Check feeds"
                                    value=refresh_val
                                    options=refresh_options.get_value()
                                    on_change=Callback::new(move |v: String| {
                                        if let Ok(n) = v.parse::<i64>() {
                                            saver.change("newsRefresh", json!(n), format!("Feeds are checked every {}", minutes(n)), true);
                                        }
                                    })
                                />
                            </SettingRow>
                            <SettingRow
                                label="Keep read articles"
                                hint="Starred articles and articles still in a feed are always kept"
                                state=saver.state("newsKeepDays")
                            >
                                <Select
                                    aria_label="Keep read articles"
                                    value=keep_val
                                    options=keep_options.get_value()
                                    on_change=Callback::new(move |v: String| {
                                        if let Ok(n) = v.parse::<i64>() {
                                            saver.change("newsKeepDays", json!(n), format!("Read articles are kept for {n} days"), true);
                                        }
                                    })
                                />
                            </SettingRow>
                        </section>
                        <section class="settings-group" aria-label="Mobile apps">
                            <h3>"Mobile apps"</h3>
                            <div class="settings-row is-stacked">
                                <div class="settings-row-text">
                                    <span class="settings-row-label">"Nextcloud News apps"</span>
                                    <small>
                                        "Use server address "<code>{addr}</code>
                                        ", your username "<code>{user}</code>
                                        " and an app password with News access."
                                    </small>
                                </div>
                                <div class="settings-row-control">
                                    <span class="news-sync">
                                        <button
                                            type="button"
                                            class="secondary-button"
                                            on:click=move |_| {
                                                copy_text(&addr_copy);
                                                saver.notice.set(Some(Notice { message: "Server address copied".into(), undo: None }));
                                            }
                                        >
                                            <Icon name="copy"/>"Copy address"
                                        </button>
                                        <a class="secondary-button" href="/settings?section=security">"App passwords"</a>
                                    </span>
                                </div>
                            </div>
                        </section>
                    }.into_any()
                }
                "Import and export" => view! {
                    <section class="settings-group" aria-label="Import and export">
                        <h3>"Subscriptions"</h3>
                        <SettingRow
                            label="Import OPML"
                            hint="Folders are kept; feeds you already follow are skipped"
                            state=Signal::derive(|| None)
                            stacked=true
                        >
                            <label class="secondary-button news-file">
                                <Icon name="upload"/>
                                {move || if importing.get() { "Importing\u{2026}" } else { "Choose OPML file" }}
                                <input
                                    type="file"
                                    accept=".opml,.xml,text/xml,text/x-opml,application/xml"
                                    disabled=move || importing.get()
                                    on:change=move |e| {
                                        if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                            let file = input.files().and_then(|list| list.item(0));
                                            input.set_value("");
                                            import_file(file);
                                        }
                                    }
                                />
                            </label>
                        </SettingRow>
                        {move || (!import_result.with(String::is_empty)).then(|| view! {
                            <p class="news-import-result" role="status">{import_result.get()}</p>
                        })}
                        {move || (!import_error.with(String::is_empty)).then(|| view! {
                            <p class="error" role="alert">{import_error.get()}</p>
                        })}
                        <SettingRow
                            label="Export OPML"
                            hint="Every feed and folder, for any other reader"
                            state=Signal::derive(|| None)
                        >
                            <a class="secondary-button" href=format!("{BASE}/opml") download=""><Icon name="download"/>"Export"</a>
                        </SettingRow>
                        <SettingRow
                            label="Export starred articles"
                            hint="Titles, links and text as JSON"
                            state=Signal::derive(|| None)
                        >
                            <a class="secondary-button" href=format!("{BASE}/starred") download=""><Icon name="download"/>"Export"</a>
                        </SettingRow>
                    </section>
                }.into_any(),
                "Keyboard shortcuts" => view! {
                    <dl class="ui-shortcuts">
                        <dt>"j / k"</dt><dd>"Next or previous article"</dd>
                        <dt>"n"</dt><dd>"Next unread article"</dd>
                        <dt>"s"</dt><dd>"Star or unstar"</dd>
                        <dt>"m"</dt><dd>"Mark read or unread"</dd>
                        <dt>"o"</dt><dd>"Open the original page"</dd>
                        <dt>"r"</dt><dd>"Update feeds"</dd>
                        <dt>"Shift A"</dt><dd>"Mark everything in this view read"</dd>
                        <dt>"Escape"</dt><dd>"Close the article"</dd>
                        <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps"</dd>
                    </dl>
                }.into_any(),
                _ => ().into_any(),
            }}
        </SettingsSheet>
    }
}
