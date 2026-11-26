//! Shared News state, API models, chronological sorting, preference helpers,
//! bulk actions, feed/folder menus, on-demand full-text fetch, and Clip to Notes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::api::{self, BrowserSession};
use crate::dates::{days_ago, local_date, local_time};
use crate::files::editor::{TextFile, write_text_file};
use crate::live::{reducer, replied};
use crate::module_bindings::create_folder_reducer::create_folder as _;
use crate::settings::Notice;
use crate::shared::context;
use crate::ui::{MenuAction, window};

pub const BASE: &str = "/api/v1/news";
pub const NEWS_REFRESH_CHOICES: [i64; 6] = [15, 30, 60, 180, 720, 1440];
pub const NEWS_KEEP_CHOICES: [i64; 4] = [7, 30, 90, 365];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsFolder {
    pub id: i64,
    pub name: String,
    pub opened: bool,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsFeed {
    pub id: i64,
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub folder_id: i64,
    #[serde(default)]
    pub added: f64,
    #[serde(default)]
    pub ordering: i64,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub full_text: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub unread_count: i64,
    #[serde(default)]
    pub last_updated: f64,
    #[serde(default)]
    pub next_update: f64,
    #[serde(default)]
    pub update_error_count: i64,
    #[serde(default)]
    pub last_update_error: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsFeedInfo {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub full_text: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub unread_count: i64,
    #[serde(default)]
    pub last_updated: f64,
    #[serde(default)]
    pub update_error_count: i64,
    #[serde(default)]
    pub last_update_error: String,
    #[serde(default)]
    pub per_day: Vec<usize>,
    #[serde(default)]
    pub starred: usize,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    pub id: i64,
    pub feed_id: i64,
    #[serde(default)]
    pub guid_hash: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub pub_date: f64,
    #[serde(default)]
    pub excerpt: String,
    #[serde(default)]
    pub enclosure_mime: String,
    #[serde(default)]
    pub enclosure_link: String,
    #[serde(default)]
    pub media_thumbnail: String,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub last_modified: f64,
    #[serde(default)]
    pub position: f64,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItemDetail {
    pub id: i64,
    pub feed_id: i64,
    #[serde(default)]
    pub guid_hash: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub pub_date: f64,
    #[serde(default)]
    pub excerpt: String,
    #[serde(default)]
    pub enclosure_mime: String,
    #[serde(default)]
    pub enclosure_link: String,
    #[serde(default)]
    pub media_thumbnail: String,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub last_modified: f64,
    #[serde(default)]
    pub position: f64,
    #[serde(default)]
    pub guid: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub updated_date: f64,
    #[serde(default)]
    pub media_description: String,
    #[serde(default)]
    pub rtl: bool,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsOverview {
    pub folders: Vec<NewsFolder>,
    pub feeds: Vec<NewsFeed>,
    pub starred_count: i64,
    pub unread_count: i64,
    pub newest_item_id: i64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsPage {
    pub items: Vec<NewsItem>,
    pub next_offset: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsDiscoveredItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub pub_date: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsDiscovered {
    pub url: String,
    pub title: String,
    pub kind: String,
    #[serde(default)]
    pub items: Vec<NewsDiscoveredItem>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DiscoverResult {
    pub items: Vec<NewsDiscovered>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MarkReadResult {
    pub changed: i64,
    #[serde(default)]
    pub ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ImportResult {
    pub added: i64,
    pub skipped: i64,
    pub folders: i64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FullTextResult {
    pub id: i64,
    pub body: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Today,
    Unread,
    Starred,
    Media,
    All,
    Feed(i64),
    Folder(i64),
}

impl View {
    pub fn api_kind(self) -> &'static str {
        match self {
            View::Unread => "unread",
            View::Starred => "starred",
            View::Today | View::Media | View::All => "all",
            View::Feed(_) => "feed",
            View::Folder(_) => "folder",
        }
    }

    pub fn id(self) -> Option<i64> {
        match self {
            View::Feed(id) | View::Folder(id) => Some(id),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListFilter {
    All,
    Unread,
    Starred,
    Media,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pane {
    Subscribe,
    Feed(NewsFeed),
    Folder(Option<NewsFolder>),
    Settings,
}

#[derive(Clone)]
pub struct ConfirmSpec {
    pub title: String,
    pub text: String,
    pub action: String,
    pub run: Arc<dyn Fn(Callback<Result<(), String>>) + Send + Sync>,
}

#[derive(Clone, Copy)]
pub struct StateChange {
    pub id: i64,
    pub feed_id: i64,
    pub unread: Option<bool>,
    pub starred: Option<bool>,
}

pub fn mark_class(id: i64) -> String {
    format!("news-mark tone-{}", id.rem_euclid(6))
}

pub fn initial(title: &str) -> String {
    title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "\u{2022}".into())
}

pub fn minutes(value: i64) -> String {
    if value >= 60 {
        format!("{} hour{}", value / 60, if value == 60 { "" } else { "s" })
    } else {
        format!("{value} minutes")
    }
}

pub fn day_label(time: f64) -> String {
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
            local_date(
                time,
                &[("day", "numeric"), ("month", "long"), ("year", "numeric")],
            )
        }
    }
}

pub fn time_label(time: f64) -> String {
    if day_label(time) == "Today" {
        local_time(time, &[("hour", "numeric"), ("minute", "2-digit")])
    } else {
        local_date(time, &[("month", "short"), ("day", "numeric")])
    }
}

pub fn relative(time: f64) -> String {
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

pub fn format_clock(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".into();
    }
    let total = seconds.floor() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn iso_string(time: f64) -> String {
    if time.is_finite() {
        js_sys::Date::new(&JsValue::from_f64(time))
            .to_iso_string()
            .as_string()
            .unwrap_or_default()
    } else {
        String::new()
    }
}

pub fn is_playable_enclosure(link: &str, mime: &str) -> bool {
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

pub fn is_video_enclosure(link: &str, mime: &str) -> bool {
    if mime.starts_with("video/") {
        return true;
    }
    let path = link.split('?').next().unwrap_or(link).to_lowercase();
    [".mp4", ".webm"].iter().any(|ext| path.ends_with(ext))
}

pub fn proxy_image_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return None;
    }
    let encoded = js_sys::encode_uri_component(trimmed).as_string()?;
    Some(format!("{BASE}/image?url={encoded}"))
}

pub fn copy_text(text: &str) {
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

pub fn has_web_share() -> bool {
    let nav = window().navigator();
    js_sys::Reflect::has(&nav, &JsValue::from_str("share")).unwrap_or(false)
}

pub fn web_share(title: &str, url: &str) {
    let nav = window().navigator();
    if let Ok(share) = js_sys::Reflect::get(&nav, &JsValue::from_str("share"))
        && let Some(func) = share.dyn_ref::<js_sys::Function>()
    {
        let data = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &data,
            &JsValue::from_str("title"),
            &JsValue::from_str(title),
        );
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

pub fn read_view() -> (View, Option<i64>) {
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
            Some("today") => View::Today,
            Some("starred") => View::Starred,
            Some("media") => View::Media,
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

pub fn write_view(view: View, item: Option<i64>) {
    let Ok(params) = web_sys::UrlSearchParams::new() else {
        return;
    };
    match view {
        View::Feed(id) => params.set("feed", &id.to_string()),
        View::Folder(id) => params.set("folder", &id.to_string()),
        View::Today => params.set("view", "today"),
        View::Starred => params.set("view", "starred"),
        View::Media => params.set("view", "media"),
        View::All => params.set("view", "all"),
        View::Unread => {}
    }
    if let Some(id) = item {
        params.set("item", &id.to_string());
    }
    let query = params.to_string().as_string().unwrap_or_default();
    let next = if query.is_empty() {
        "/news".to_owned()
    } else {
        format!("/news?{query}")
    };
    let loc = window().location();
    let current = format!(
        "{}{}",
        loc.pathname().unwrap_or_default(),
        loc.search().unwrap_or_default()
    );
    if next != current
        && let Ok(history) = window().history()
    {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&next));
    }
}

pub fn items_query(view: View, order: &str, show_read: bool, offset: i64) -> String {
    let params = web_sys::UrlSearchParams::new().expect("params");
    params.set("type", view.api_kind());
    let batch = match view {
        View::Today | View::Media => "200",
        _ => "60",
    };
    params.set("batch", batch);
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
    format!(
        "{BASE}/items?{}",
        params.to_string().as_string().unwrap_or_default()
    )
}

/// Chronologically sorts and filters articles so day headers (`Today`, `Yesterday`, ...)
/// never repeat out of order across feeds.
pub fn visible_items(
    items: &[NewsItem],
    view: View,
    filter: ListFilter,
    order: &str,
) -> Vec<NewsItem> {
    let mut out: Vec<NewsItem> = items
        .iter()
        .filter(|i| match view {
            View::Today => days_ago(i.pub_date) <= 0,
            View::Media => !i.enclosure_link.is_empty(),
            _ => true,
        })
        .filter(|i| match filter {
            ListFilter::All => true,
            ListFilter::Unread => i.unread,
            ListFilter::Starred => i.starred,
            ListFilter::Media => !i.enclosure_link.is_empty(),
        })
        .cloned()
        .collect();
    if order == "oldest" {
        out.sort_by(|a, b| a.pub_date.total_cmp(&b.pub_date).then(a.id.cmp(&b.id)));
    } else {
        out.sort_by(|a, b| b.pub_date.total_cmp(&a.pub_date).then(b.id.cmp(&a.id)));
    }
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotesFolder {
    id: String,
    parent_id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotesIndex {
    root_id: String,
    #[serde(default)]
    folders: Vec<NotesFolder>,
}

fn clip_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut name = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    name = name
        .trim_matches('.')
        .trim()
        .chars()
        .take(110)
        .collect::<String>()
        .trim()
        .to_owned();
    if name.is_empty() {
        name = "Clipped article".into();
    }
    format!("{name}.md")
}

#[derive(Clone, Copy)]
pub struct NewsState {
    pub overview: RwSignal<Option<NewsOverview>>,
    pub load_error: RwSignal<String>,
    pub view_sig: RwSignal<View>,
    pub items: RwSignal<Vec<NewsItem>>,
    pub next_offset: RwSignal<Option<i64>>,
    pub list_state: RwSignal<&'static str>,
    pub list_error: RwSignal<String>,
    pub filter: RwSignal<ListFilter>,
    pub checked: RwSignal<HashSet<i64>>,
    pub sidebar_unread_only: RwSignal<bool>,
    pub focus_mode: RwSignal<bool>,
    pub aa_open: RwSignal<bool>,
    pub feed_info: RwSignal<Option<NewsFeedInfo>>,
    pub selected: RwSignal<Option<i64>>,
    pub article: RwSignal<Option<NewsItemDetail>>,
    pub article_error: RwSignal<String>,
    pub fulltext_loading: RwSignal<bool>,
    pub fulltext_fetched: RwSignal<Option<i64>>,
    pub pane: RwSignal<Option<Pane>>,
    pub confirm: RwSignal<Option<ConfirmSpec>>,
    pub notice: RwSignal<Option<Notice>>,
    pub busy: RwSignal<bool>,
    pub refreshing: RwSignal<bool>,
    pub navigation: RwSignal<bool>,
    pub list_ref: NodeRef<html::Div>,
    pub generation: RwSignal<u32>,
    pub pending_read: StoredValue<HashSet<i64>>,
    pub session: StoredValue<BrowserSession>,
    pub news_mark_read: Memo<String>,
    pub news_order: Memo<String>,
    pub news_show_read: Memo<bool>,
    pub news_images: Memo<String>,
    pub news_layout: Memo<String>,
    pub news_paper: Memo<String>,
    pub news_font: Memo<String>,
    pub news_size: Memo<String>,
}

impl NewsState {
    pub fn csrf(self) -> String {
        self.session.with_value(|s| s.csrf_token.clone())
    }

    pub fn notify(self, message: impl Into<String>) {
        let _ = self.notice.try_set(Some(Notice {
            message: message.into(),
            undo: None,
        }));
    }

    pub fn fail(self, message: String) {
        let msg = if message.is_empty() {
            "The request could not be completed.".into()
        } else {
            message
        };
        self.notify(msg);
    }

    /// Persists a single News display/reading preference back to account preferences.
    pub fn save_pref(self, key: &'static str, value: Value) {
        let prefs = context().preferences;
        let session = self.session.get_value();
        spawn_local(async move {
            let rev = prefs
                .value
                .with_untracked(|p| p.as_ref().map(|p| p.revision))
                .unwrap_or(0);
            let mut map = serde_json::Map::new();
            map.insert(key.into(), value.clone());
            // Optimistically update local preferences store for instant UI feedback.
            let _ = prefs.value.try_update(|slot| {
                if let Some(p) = slot {
                    p.rest.insert(key.into(), value.clone());
                }
            });
            match crate::shared::save_preferences(Value::Object(map), rev, &session).await {
                Ok(updated) => {
                    let _ = prefs.value.try_set(Some(updated));
                }
                Err(_) => {
                    prefs.refresh();
                }
            }
        });
    }

    pub async fn fetch_overview(self) {
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

    pub fn load_overview(self) {
        spawn_local(async move {
            self.fetch_overview().await;
        });
    }

    pub fn load_feed_info(self, feed_id: i64) {
        let _ = self.feed_info.try_set(None);
        spawn_local(async move {
            if let Ok(info) = api::get::<NewsFeedInfo>(&format!("{BASE}/feeds/{feed_id}")).await
                && self.view_sig.try_get_untracked() == Some(View::Feed(feed_id))
            {
                let _ = self.feed_info.try_set(Some(info));
            }
        });
    }

    pub async fn fetch_items(self) {
        let Some(prev) = self.generation.try_get_untracked() else {
            return;
        };
        let ticket = prev + 1;
        let _ = self.generation.try_set(ticket);
        let _ = self.list_state.try_set("loading");
        let _ = self.checked.try_set(HashSet::new());
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

    pub fn load_items(self) {
        spawn_local(async move {
            self.fetch_items().await;
        });
    }

    pub fn load_more(self) {
        let Some(offset) = self.next_offset.get_untracked() else {
            return;
        };
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
            let Ok(page) = api::get::<NewsPage>(&url).await else {
                return;
            };
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
    pub fn adjust(self, changes: &[StateChange]) {
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

    pub fn set_state(self, targets: Vec<(i64, i64)>, unread: Option<bool>, starred: Option<bool>) {
        if targets.is_empty() {
            return;
        }
        let forward: Vec<StateChange> = targets
            .iter()
            .map(|&(id, feed_id)| StateChange {
                id,
                feed_id,
                unread,
                starred,
            })
            .collect();
        self.adjust(&forward);
        let mut body = serde_json::Map::new();
        body.insert(
            "ids".into(),
            json!(targets.iter().map(|(id, _)| *id).collect::<Vec<_>>()),
        );
        if let Some(u) = unread {
            body.insert("unread".into(), json!(u));
        }
        if let Some(s) = starred {
            body.insert("starred".into(), json!(s));
        }
        let csrf = self.csrf();
        spawn_local(async move {
            if let Err(e) = api::send::<Value>(
                &format!("{BASE}/items/state"),
                "POST",
                Some(Value::Object(body)),
                &csrf,
            )
            .await
            {
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

    pub fn open(self, id: i64) {
        self.selected.set(Some(id));
        self.article_error.set(String::new());
        self.fulltext_loading.set(false);
        self.fulltext_fetched.set(None);
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
                    let should_mark =
                        detail.unread && self.news_mark_read.get_untracked() == "open";
                    let feed_id = detail.feed_id;
                    let _ = self.article.try_set(Some(detail));
                    if should_mark {
                        self.adjust(&[StateChange {
                            id,
                            feed_id,
                            unread: Some(false),
                            starred: None,
                        }]);
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

    /// Fetches the current article's original page on demand and extracts its main prose.
    pub fn fetch_fulltext(self) {
        let Some(art) = self.article.get_untracked() else {
            return;
        };
        if self.fulltext_loading.get_untracked() {
            return;
        }
        let id = art.id;
        let csrf = self.csrf();
        self.fulltext_loading.set(true);
        spawn_local(async move {
            match api::send::<FullTextResult>(
                &format!("{BASE}/items/{id}/fulltext"),
                "POST",
                Some(json!({})),
                &csrf,
            )
            .await
            {
                Ok(res) => {
                    let _ = self.article.try_update(|a| {
                        if let Some(a) = a
                            && a.id == res.id
                        {
                            a.body = res.body;
                        }
                    });
                    let _ = self.fulltext_fetched.try_set(Some(id));
                    self.notify("Full article text loaded");
                }
                Err(e) => self.fail(e.message),
            }
            let _ = self.fulltext_loading.try_set(false);
        });
    }

    /// Clips the current article to `Clippings/<title>.md` in Dar Notes.
    pub fn clip_to_notes(self, feed_title: String, plain_text: String) {
        let Some(art) = self.article.get_untracked() else {
            return;
        };
        let sess = self.session.get_value();
        spawn_local(async move {
            let index = match api::get::<NotesIndex>("/api/v1/notes/library").await {
                Ok(idx) => idx,
                Err(e) => {
                    self.fail(format!("Could not reach Notes: {}", e.message));
                    return;
                }
            };
            let folder = index
                .folders
                .iter()
                .find(|f| f.parent_id == index.root_id && f.name.eq_ignore_ascii_case("Clippings"));
            let folder_id = match folder {
                Some(f) => f.id.clone(),
                None => {
                    let Some(conn) = crate::live::connection() else {
                        self.fail("Dar is reconnecting. Try again in a moment.".into());
                        return;
                    };
                    let new_id = crate::random_uuid();
                    let (fid, parent) = (new_id.clone(), index.root_id.clone());
                    if let Err(e) = reducer(|tx| {
                        conn.reducers
                            .create_folder_then(fid, parent, "Clippings".into(), replied(tx))
                    })
                    .await
                    {
                        self.fail(e);
                        return;
                    }
                    new_id
                }
            };
            let title = if art.title.is_empty() {
                "Clipped article"
            } else {
                &art.title
            };
            let mut md = format!("# {title}\n\n");
            if !feed_title.is_empty() {
                md.push_str(&format!("- **Source:** {feed_title}\n"));
            }
            if !art.author.is_empty() {
                md.push_str(&format!("- **Author:** {}\n", art.author));
            }
            if !art.url.is_empty() {
                md.push_str(&format!("- **Original:** <{}>\n", art.url));
            }
            md.push_str(&format!("- **In Dar News:** [Open article](/news?item={})\n\n", art.id));
            let body_text = plain_text.trim();
            if !body_text.is_empty() {
                md.push_str(body_text);
                md.push('\n');
            }
            let filename = clip_filename(title);
            let file = TextFile {
                name: &filename,
                parent_id: &folder_id,
                content: &md,
                media_type: "text/markdown",
                file_id: None,
                expected_revision: None,
            };
            match write_text_file(&sess, file).await {
                Ok(_) => self.notify(format!("Saved to Notes → Clippings/{filename}")),
                Err(e) => self.fail(e.message),
            }
        });
    }

    pub fn visible_list(self) -> Vec<NewsItem> {
        let items = self.items.get_untracked();
        let view = self.view_sig.get_untracked();
        let filter = self.filter.get_untracked();
        let order = self.news_order.get_untracked();
        visible_items(&items, view, filter, &order)
    }

    pub fn step(self, delta: i32, unread_only: bool) {
        let list = self.visible_list();
        let sel = self.selected.get_untracked();
        let index = sel.and_then(|s| list.iter().position(|i| i.id == s));
        let next = if delta > 0 {
            let start = index.map(|i| i + 1).unwrap_or(0);
            list.get(start..)
                .unwrap_or(&[])
                .iter()
                .find(|i| !unread_only || i.unread)
                .cloned()
        } else {
            let end = index.unwrap_or(0);
            list.get(..end)
                .unwrap_or(&[])
                .iter()
                .rev()
                .find(|i| !unread_only || i.unread)
                .cloned()
        };
        if let Some(next) = next {
            let next_id = next.id;
            self.open(next_id);
            let list_ref = self.list_ref;
            let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
                if let Some(el) = list_ref.get_untracked().and_then(|l| {
                    l.query_selector(&format!("[data-item=\"{next_id}\"]"))
                        .ok()
                        .flatten()
                }) {
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

    pub fn mark_all(self) {
        let items_max = self
            .items
            .with_untracked(|list| list.iter().map(|i| i.id).max().unwrap_or(0));
        let overview_max = self
            .overview
            .with_untracked(|o| o.as_ref().map(|o| o.newest_item_id).unwrap_or(0));
        let newest = 0i64.max(items_max).max(overview_max);
        let view = self.view_sig.get_untracked();
        // Today & Media are filtered client subsets: mark only the visible unread items.
        if matches!(view, View::Today | View::Media) {
            let targets: Vec<(i64, i64)> = self
                .visible_list()
                .into_iter()
                .filter(|i| i.unread)
                .map(|i| (i.id, i.feed_id))
                .collect();
            let count = targets.len();
            if count == 0 {
                return;
            }
            let undo_targets = targets.clone();
            self.set_state(targets, Some(false), None);
            let undo = Arc::new(move || {
                self.set_state(undo_targets.clone(), Some(true), None);
            }) as Arc<dyn Fn() + Send + Sync>;
            let _ = self.notice.try_set(Some(Notice {
                message: format!(
                    "{count} article{} marked read",
                    if count == 1 { "" } else { "s" }
                ),
                undo: Some(undo),
            }));
            return;
        }
        let mut body = serde_json::Map::new();
        body.insert("type".into(), json!(view.api_kind()));
        if let Some(id) = view.id() {
            body.insert("id".into(), json!(id));
        }
        body.insert("newestItemId".into(), json!(newest));
        self.busy.set(true);
        let csrf = self.csrf();
        spawn_local(async move {
            match api::send::<MarkReadResult>(
                &format!("{BASE}/read"),
                "POST",
                Some(Value::Object(body)),
                &csrf,
            )
            .await
            {
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

    pub fn refresh(self) {
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

    pub fn update_feed(self, feed_id: i64, change: Value, message: Option<&'static str>) {
        let reload_items = change.get("updateNow").and_then(Value::as_bool) == Some(true)
            || change.get("paused").and_then(Value::as_bool) == Some(false);
        let csrf = self.csrf();
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/feeds/{feed_id}"),
                "PATCH",
                Some(change),
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    self.fetch_overview().await;
                    if reload_items {
                        self.fetch_items().await;
                    }
                    if self.view_sig.try_get_untracked() == Some(View::Feed(feed_id)) {
                        self.load_feed_info(feed_id);
                    }
                    if let Some(msg) = message {
                        self.notify(msg);
                    }
                }
                Err(e) => self.fail(e.message),
            }
        });
    }

    pub fn choose(self, next: View) {
        self.view_sig.set(next);
        self.selected.set(None);
        self.article.set(None);
        self.focus_mode.set(false);
        self.aa_open.set(false);
        self.filter.set(ListFilter::All);
        self.checked.set(HashSet::new());
        self.navigation.set(false);
        if let View::Feed(id) = next {
            self.load_feed_info(id);
        } else {
            self.feed_info.set(None);
        }
    }

    pub fn feed_actions(self, feed: NewsFeed) -> Vec<MenuAction> {
        let feed_id = feed.id;
        let feed_title = feed.title.clone();
        let feed_edit = feed.clone();
        let full_text = feed.full_text;
        let pinned = feed.pinned;
        let paused = feed.paused;
        vec![
            MenuAction::new("Mark all read", "checks").run(move || {
                self.choose(View::Feed(feed_id));
                let newest = self
                    .overview
                    .with_untracked(|o| o.as_ref().map(|o| o.newest_item_id).unwrap_or(0));
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
            MenuAction::new(
                if full_text {
                    "Full text: on"
                } else {
                    "Full text: off"
                },
                "file",
            )
            .run(move || {
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
            MenuAction::new(
                if paused {
                    "Resume updates"
                } else {
                    "Pause updates"
                },
                "pause",
            )
            .run(move || {
                self.update_feed(
                    feed_id,
                    json!({ "paused": !paused }),
                    Some(if paused {
                        "Updates resumed"
                    } else {
                        "Updates paused"
                    }),
                );
            }),
            MenuAction::new("Update now", "refresh")
                .disabled(paused)
                .run(move || {
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

    pub fn folder_actions(self, folder: NewsFolder) -> Vec<MenuAction> {
        let folder_id = folder.id;
        let folder_name = folder.name.clone();
        let folder_edit = folder.clone();
        vec![
            MenuAction::new("Mark all read", "checks").run(move || {
                let newest = self
                    .overview
                    .with_untracked(|o| o.as_ref().map(|o| o.newest_item_id).unwrap_or(0));
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
                self.pane
                    .set(Some(Pane::Folder(Some(folder_edit.clone()))));
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
