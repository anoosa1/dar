//! News: folders, feeds and items with integer IDs that Nextcloud News clients sync (`news.ts`).

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::app_settings::app_enabled;
use crate::tables::*;
use crate::{Res, fail, now, row};

type Doc = Map<String, Value>;

const FOLDERS: usize = 100;
const FEEDS: usize = 500;
const ITEMS_PER_FEED: usize = 1000;
const ITEMS_PER_ACCOUNT: u32 = 60000;
const PAGE: f64 = 100.0;
const TITLE: usize = 1000;
const BODY: usize = 200 * 1024;
const URL: usize = 2048;
const REFRESH_CHOICES: [f64; 6] = [15.0, 30.0, 60.0, 180.0, 720.0, 1440.0];
const KEEP_CHOICES: [f64; 4] = [7.0, 30.0, 90.0, 365.0];
const PAUSED: f64 = 1e15;
pub const READ_OPERATIONS: [&str; 9] = ["access", "overview", "folders", "feeds", "items", "itemsUpdated", "item", "exportData", "feedInfo"];

fn text(value: Option<&Value>, max: usize, allow_empty: bool) -> Res<String> {
    let Some(s) = value.and_then(Value::as_str) else { return fail("invalidInput") };
    if !allow_empty && s.trim().is_empty() || s.contains('\0') || s.encode_utf16().count() > max {
        return fail("invalidInput");
    }
    Ok(s.to_owned())
}

fn int(value: Option<&Value>, min: f64, max: f64) -> Res<u32> {
    match value.and_then(Value::as_f64) {
        Some(f) if f.fract() == 0.0 && f >= min && f <= max => Ok(f as u32),
        _ => fail("invalidInput"),
    }
}

fn keys(p: &Doc, allowed: &[&str]) -> Res<()> {
    if p.keys().any(|k| !allowed.contains(&k.as_str())) { fail("invalidInput") } else { Ok(()) }
}

fn url(value: Option<&Value>) -> Res<String> {
    let v = text(value, URL, false)?;
    let lower = v.to_lowercase();
    let rest = lower.strip_prefix("http://").or_else(|| lower.strip_prefix("https://"));
    if rest.is_none_or(|r| r.is_empty() || r.chars().any(char::is_whitespace)) {
        return fail("invalidInput");
    }
    Ok(v)
}

fn optional_url(value: Option<&Value>) -> Res<String> {
    if value.and_then(Value::as_str) == Some("") { Ok(String::new()) } else { url(value) }
}

/// `prefix(value)`: keys under `value/`.
fn prefix(value: &str) -> (std::ops::Bound<String>, std::ops::Bound<String>) {
    (std::ops::Bound::Included(format!("{value}/")), std::ops::Bound::Excluded(format!("{value}0")))
}

fn feed_prefix(id: u32) -> String {
    format!("{id:010}")
}

pub fn news_key(id: u32) -> String {
    format!("{:010}", 4_294_967_295u64 - id as u64)
}

fn modified_key(owner_id: &str, micros: f64) -> String {
    format!("{owner_id}/{:017}", micros as u64)
}

fn micros(ctx: &ReducerContext) -> f64 {
    ctx.timestamp.to_micros_since_unix_epoch() as f64
}

fn due_key(at: f64) -> String {
    format!("{:016}", at.floor().clamp(0.0, 9_999_999_999_999_999.0) as u64)
}

fn next_id(ctx: &ReducerContext, name: &str) -> Res<u32> {
    let old = ctx.db.news_sequences().name().find(name.to_owned());
    let value = old.as_ref().map_or(0, |r| r.value) as u64 + 1;
    if value >= 0xffff_fffe {
        return fail("quotaExceeded");
    }
    let next = NewsSequence { name: name.into(), value: value as u32 };
    if old.is_some() {
        ctx.db.news_sequences().name().update(next);
    } else {
        ctx.db.news_sequences().insert(next);
    }
    Ok(value as u32)
}

/// The account's refresh interval and retention, in milliseconds.
pub fn news_preferences(ctx: &ReducerContext, user_id: &str) -> (f64, f64) {
    let value: Value = ctx.db.pim_preferences().user_id().find(user_id.to_owned()).and_then(|r| serde_json::from_str(&r.value).ok()).unwrap_or(json!({}));
    let refresh = value["newsRefresh"].as_f64().filter(|v| REFRESH_CHOICES.contains(v)).unwrap_or(30.0);
    let keep = value["newsKeepDays"].as_f64().filter(|v| KEEP_CHOICES.contains(v)).unwrap_or(30.0);
    (refresh * 60000.0, keep * 86_400_000.0)
}

fn adjust_count(ctx: &ReducerContext, owner_id: &str, delta: i64) {
    let old = ctx.db.news_counts().owner_id().find(owner_id.to_owned());
    let items = (old.as_ref().map_or(0, |r| r.items) as i64 + delta).max(0) as u32;
    let next = NewsCount { owner_id: owner_id.into(), items };
    if old.is_some() {
        ctx.db.news_counts().owner_id().update(next);
    } else {
        ctx.db.news_counts().insert(next);
    }
}

fn expose_item(r: &NewsItem) -> Value {
    json!({ "id": r.id, "feedId": r.feed_id, "guidHash": r.guid_hash, "url": r.url, "title": r.title, "author": r.author, "pubDate": r.pub_date, "excerpt": r.excerpt,
        "enclosureMime": r.enclosure_mime, "enclosureLink": r.enclosure_link, "mediaThumbnail": r.media_thumbnail, "unread": r.unread, "starred": r.starred,
        "lastModified": r.last_modified, "position": r.position })
}

fn detail(ctx: &ReducerContext, r: &NewsItem) -> Value {
    let mut v = expose_item(r);
    let o = v.as_object_mut().expect("item");
    o.insert("guid".into(), json!(r.guid));
    o.insert("body".into(), json!(ctx.db.news_bodies().item_id().find(r.id).map(|b| b.body).unwrap_or_default()));
    o.insert("updatedDate".into(), json!(r.updated_date));
    o.insert("mediaDescription".into(), json!(r.media_description));
    o.insert("rtl".into(), json!(r.rtl));
    o.insert("fingerprint".into(), json!(r.fingerprint));
    o.insert("contentHash".into(), json!(r.content_hash));
    v
}

fn set_state(ctx: &ReducerContext, r: &NewsItem, unread: Option<bool>, starred: Option<bool>, position: Option<f64>) -> NewsItem {
    let unread = unread.unwrap_or(r.unread);
    let starred = starred.unwrap_or(r.starred);
    let position = position.unwrap_or(r.position);
    if unread == r.unread && starred == r.starred && position == r.position {
        return r.clone();
    }
    let last_modified = micros(ctx);
    let next = NewsItem {
        unread,
        starred,
        position,
        unread_key: if unread { r.owner_id.clone() } else { String::new() },
        starred_key: if starred { r.owner_id.clone() } else { String::new() },
        last_modified,
        modified_key: modified_key(&r.owner_id, last_modified),
        ..r.clone()
    };
    ctx.db.news_items().id().update(next.clone());
    next
}

fn delete_item(ctx: &ReducerContext, r: &NewsItem) {
    ctx.db.news_bodies().item_id().delete(r.id);
    ctx.db.news_items().id().delete(r.id);
    adjust_count(ctx, &r.owner_id, -1);
}

fn feed_items(ctx: &ReducerContext, feed_id: u32) -> Vec<NewsItem> {
    let range = prefix(&feed_prefix(feed_id));
    ctx.db.news_items().feed_key().filter(crate::bounds(&range)).collect()
}

fn delete_feed_rows(ctx: &ReducerContext, feed: &NewsFeed) {
    for r in feed_items(ctx, feed.id) {
        delete_item(ctx, &r);
    }
    ctx.db.news_feeds().id().delete(feed.id);
}

fn hex32(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|s| s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

struct Entry {
    item: NewsItem,
    body: String,
}

fn valid_entry(value: &Value) -> Res<Entry> {
    let Some(e) = value.as_object() else { return fail("invalidInput") };
    keys(e, &["guid", "guidHash", "url", "title", "author", "pubDate", "updatedDate", "body", "enclosureMime", "enclosureLink", "mediaThumbnail", "mediaDescription", "rtl", "fingerprint", "contentHash", "excerpt"])?;
    if !e.get("rtl").is_some_and(Value::is_boolean) || !hex32(e.get("guidHash")) || !hex32(e.get("fingerprint")) || !hex32(e.get("contentHash")) {
        return fail("invalidInput");
    }
    for k in ["pubDate", "updatedDate"] {
        if !e.get(k).and_then(Value::as_f64).is_some_and(|f| f.is_finite() && f >= 0.0) {
            return fail("invalidInput");
        }
    }
    let item_url = if e.get("url").and_then(Value::as_str) == Some("") { String::new() } else { url(e.get("url"))? };
    Ok(Entry {
        item: NewsItem {
            id: 0,
            owner_id: String::new(),
            feed_id: 0,
            dedupe_key: String::new(),
            owner_key: String::new(),
            feed_key: String::new(),
            unread_key: String::new(),
            starred_key: String::new(),
            modified_key: String::new(),
            guid: text(e.get("guid"), URL, false)?,
            guid_hash: e["guidHash"].as_str().unwrap_or_default().into(),
            url: item_url,
            title: text(e.get("title"), TITLE, true)?,
            author: text(e.get("author"), 500, true)?,
            pub_date: e["pubDate"].as_f64().unwrap_or(0.0),
            updated_date: e["updatedDate"].as_f64().unwrap_or(0.0),
            excerpt: text(Some(e.get("excerpt").unwrap_or(&json!(""))), 600, true)?,
            enclosure_mime: text(e.get("enclosureMime"), 200, true)?,
            enclosure_link: optional_url(e.get("enclosureLink"))?,
            media_thumbnail: optional_url(e.get("mediaThumbnail"))?,
            media_description: text(e.get("mediaDescription"), 4000, true)?,
            rtl: e["rtl"].as_bool().unwrap_or(false),
            fingerprint: e["fingerprint"].as_str().unwrap_or_default().into(),
            content_hash: e["contentHash"].as_str().unwrap_or_default().into(),
            unread: true,
            starred: false,
            last_modified: 0.0,
            position: 0.0,
            fetched_at: 0.0,
        },
        body: text(e.get("body"), BODY, true)?,
    })
}

/// Inserts new entries (oldest first, so IDs follow time) and refreshes changed ones without
/// touching read or starred state. Retention never deletes starred items or entries still in
/// the feed, which would otherwise return as unread on the next update.
fn store_entries(ctx: &ReducerContext, feed: &NewsFeed, input: &Value, all: Option<&Value>) -> Res<u32> {
    let Some(input) = input.as_array().filter(|a| a.len() <= ITEMS_PER_FEED) else { return fail("invalidInput") };
    let all: Vec<String> = match all {
        None => Vec::new(),
        Some(v) => {
            let Some(list) = v.as_array().filter(|a| a.len() <= ITEMS_PER_FEED) else { return fail("invalidInput") };
            if list.iter().any(|h| !hex32(Some(h))) {
                return fail("invalidInput");
            }
            list.iter().filter_map(|h| h.as_str().map(str::to_owned)).collect()
        }
    };
    let mut entries = input.iter().map(valid_entry).collect::<Res<Vec<_>>>()?;
    entries.sort_by(|a, b| a.item.pub_date.total_cmp(&b.item.pub_date));
    let present: HashSet<String> = entries.iter().map(|e| e.item.guid_hash.clone()).chain(all).collect();
    let at = now(ctx);
    let stamp = micros(ctx);
    let mut added = 0u32;
    let count = ctx.db.news_counts().owner_id().find(feed.owner_id.clone()).map_or(0, |r| r.items);
    for entry in entries {
        let dedupe_key = format!("{}:{}", feed.id, entry.item.guid_hash);
        if let Some(old) = ctx.db.news_items().dedupe_key().find(dedupe_key.clone()) {
            if old.content_hash == entry.item.content_hash {
                continue;
            }
            let meta = entry.item;
            ctx.db.news_items().id().update(NewsItem {
                guid: meta.guid,
                guid_hash: meta.guid_hash,
                url: meta.url,
                title: meta.title,
                author: meta.author,
                pub_date: meta.pub_date,
                updated_date: meta.updated_date,
                excerpt: meta.excerpt,
                enclosure_mime: meta.enclosure_mime,
                enclosure_link: meta.enclosure_link,
                media_thumbnail: meta.media_thumbnail,
                media_description: meta.media_description,
                rtl: meta.rtl,
                fingerprint: meta.fingerprint,
                content_hash: meta.content_hash,
                last_modified: stamp,
                modified_key: modified_key(&feed.owner_id, stamp),
                ..old.clone()
            });
            let stored = NewsBody { item_id: old.id, body: entry.body };
            if ctx.db.news_bodies().item_id().find(old.id).is_some() {
                ctx.db.news_bodies().item_id().update(stored);
            } else {
                ctx.db.news_bodies().insert(stored);
            }
            continue;
        }
        if count + added >= ITEMS_PER_ACCOUNT {
            break;
        }
        let id = next_id(ctx, "item")?;
        added += 1;
        ctx.db.news_items().insert(NewsItem {
            id,
            owner_id: feed.owner_id.clone(),
            feed_id: feed.id,
            dedupe_key,
            owner_key: format!("{}/{}", feed.owner_id, news_key(id)),
            feed_key: format!("{}/{}", feed_prefix(feed.id), news_key(id)),
            unread_key: feed.owner_id.clone(),
            starred_key: String::new(),
            modified_key: modified_key(&feed.owner_id, stamp),
            unread: true,
            starred: false,
            last_modified: stamp,
            position: 0.0,
            fetched_at: at,
            ..entry.item
        });
        ctx.db.news_bodies().insert(NewsBody { item_id: id, body: entry.body });
    }
    if added > 0 {
        adjust_count(ctx, &feed.owner_id, added as i64);
    }
    let (_, keep_ms) = news_preferences(ctx, &feed.owner_id);
    let mut kept = 0usize;
    for r in feed_items(ctx, feed.id) {
        if r.starred || present.contains(&r.guid_hash) {
            kept += 1;
            continue;
        }
        kept += 1;
        if kept > ITEMS_PER_FEED || !r.unread && r.fetched_at < at - keep_ms {
            delete_item(ctx, &r);
        }
    }
    Ok(added)
}

fn expose_feed(r: &NewsFeed, unread: &HashMap<u32, u32>) -> Value {
    json!({ "id": r.id, "url": r.url, "title": r.title, "link": r.link, "folderId": r.folder_id, "added": r.added, "ordering": r.ordering, "pinned": r.pinned,
        "fullText": r.full_text, "paused": r.paused, "unreadCount": unread.get(&r.id).copied().unwrap_or(0), "lastUpdated": r.last_updated, "nextUpdate": r.next_update,
        "updateErrorCount": r.error_count, "lastUpdateError": r.last_error })
}

fn unread_counts(ctx: &ReducerContext, user_id: &str) -> HashMap<u32, u32> {
    let mut counts = HashMap::new();
    for r in ctx.db.news_items().unread_key().filter(user_id) {
        *counts.entry(r.feed_id).or_insert(0) += 1;
    }
    counts
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

pub fn news_operation(ctx: &ReducerContext, user_id: &str, operation: &str, p: &Doc) -> Res<Value> {
    if !app_enabled(ctx, "news") {
        return fail("notFound");
    }
    let at = now(ctx);
    let folder = |value: Option<&Value>| -> Res<NewsFolder> {
        let id = int(value, 1.0, 4_294_967_295.0)?;
        ctx.db.news_folders().id().find(id).filter(|f| f.owner_id == user_id).ok_or_else(|| "notFound".to_owned())
    };
    let feed = |value: Option<&Value>| -> Res<NewsFeed> {
        let id = int(value, 1.0, 4_294_967_295.0)?;
        ctx.db.news_feeds().id().find(id).filter(|f| f.owner_id == user_id).ok_or_else(|| "notFound".to_owned())
    };
    let folder_id = |value: Option<&Value>| -> Res<u32> {
        match value {
            None | Some(Value::Null) => Ok(0),
            Some(v) if v.as_f64() == Some(0.0) => Ok(0),
            v => Ok(folder(v)?.id),
        }
    };
    let name = |value: Option<&Value>| -> Res<String> { Ok(text(value, 200, false)?.trim().to_owned()) };
    let feeds = || -> Vec<NewsFeed> { ctx.db.news_feeds().owner_id().filter(user_id).collect() };
    let in_scope = |scope: &Doc| -> Res<Box<dyn Fn(&NewsItem) -> bool + '_>> {
        match scope.get("type").and_then(Value::as_str) {
            Some("feed") => {
                let id = feed(scope.get("id"))?.id;
                Ok(Box::new(move |r: &NewsItem| r.feed_id == id))
            }
            Some("folder") => {
                let target = if scope.get("id").and_then(Value::as_f64) == Some(0.0) { 0 } else { folder(scope.get("id"))?.id };
                let ids: HashSet<u32> = feeds().into_iter().filter(|f| f.folder_id == target).map(|f| f.id).collect();
                Ok(Box::new(move |r: &NewsItem| ids.contains(&r.feed_id)))
            }
            Some("starred") => Ok(Box::new(|r: &NewsItem| r.starred)),
            Some("all") => Ok(Box::new(|_: &NewsItem| true)),
            Some("unread") => Ok(Box::new(|r: &NewsItem| r.unread)),
            _ => fail("invalidInput"),
        }
    };
    match operation {
        "access" => {
            keys(p, &[])?;
            Ok(json!({ "ok": true }))
        }
        "overview" | "folders" | "feeds" => {
            keys(p, &[])?;
            let counts = unread_counts(ctx, user_id);
            let mut list = feeds();
            list.sort_by(|a, b| b.pinned.cmp(&a.pinned).then_with(|| crate::locale_compare(&a.title, &b.title)).then(a.id.cmp(&b.id)));
            let range = prefix(user_id);
            let newest = ctx.db.news_items().owner_key().filter(crate::bounds(&range)).next().map_or(0, |r| r.id);
            let mut folders: Vec<NewsFolder> = ctx.db.news_folders().owner_id().filter(user_id).collect();
            folders.sort_by(|a, b| crate::locale_compare(&a.name, &b.name).then(a.id.cmp(&b.id)));
            Ok(json!({
                "folders": folders.iter().map(|f| json!({ "id": f.id, "name": f.name, "opened": f.opened })).collect::<Vec<_>>(),
                "feeds": list.iter().map(|f| expose_feed(f, &counts)).collect::<Vec<_>>(),
                "starredCount": ctx.db.news_items().starred_key().filter(user_id).count(),
                "unreadCount": counts.values().sum::<u32>(),
                "newestItemId": newest,
            }))
        }
        "items" => {
            keys(p, &["type", "id", "offset", "batch", "getRead", "oldestFirst", "bodies"])?;
            let matches = in_scope(p)?;
            let get_read = p.get("getRead") != Some(&json!(false));
            let oldest = p.get("oldestFirst") == Some(&json!(true));
            let offset = match p.get("offset") {
                None => 0,
                v => int(v, 0.0, 4_294_967_295.0)?,
            };
            let batch = if p.get("batch").and_then(Value::as_f64) == Some(-1.0) { 2000 } else { int(Some(p.get("batch").unwrap_or(&json!(PAGE))), 1.0, 2000.0)? } as usize;
            let kind = p.get("type").and_then(Value::as_str).unwrap_or_default();
            let source: Vec<NewsItem> = if kind == "unread" || !get_read && kind != "starred" {
                let mut v: Vec<NewsItem> = ctx.db.news_items().unread_key().filter(user_id).collect();
                v.sort_by(|a, b| b.id.cmp(&a.id));
                v
            } else if kind == "starred" {
                let mut v: Vec<NewsItem> = ctx.db.news_items().starred_key().filter(user_id).collect();
                v.sort_by(|a, b| b.id.cmp(&a.id));
                v
            } else if kind == "feed" {
                feed_items(ctx, feed(p.get("id"))?.id)
            } else {
                let range = prefix(user_id);
                ctx.db.news_items().owner_key().filter(crate::bounds(&range)).collect()
            };
            let mut rows: Vec<NewsItem> = Vec::new();
            for r in source {
                if !matches(&r) || !get_read && !r.unread {
                    continue;
                }
                if !oldest && offset > 0 && r.id >= offset {
                    continue;
                }
                rows.push(r);
                if !oldest && rows.len() > batch {
                    break;
                }
            }
            if oldest {
                rows.reverse();
                rows.retain(|r| offset == 0 || r.id > offset);
            }
            let page: Vec<&NewsItem> = rows.iter().take(batch).collect();
            let bodies = p.get("bodies") == Some(&json!(true));
            let next = if rows.len() > batch { json!(page.last().map(|r| r.id)) } else { Value::Null };
            Ok(json!({ "items": page.iter().map(|r| if bodies { detail(ctx, r) } else { expose_item(r) }).collect::<Vec<_>>(), "nextOffset": next }))
        }
        "itemsUpdated" => {
            keys(p, &["type", "id", "lastModified"])?;
            let Some(since) = p.get("lastModified").and_then(Value::as_f64).filter(|f| f.is_finite() && *f >= 0.0) else { return fail("invalidInput") };
            let matches = in_scope(p)?;
            let range = (std::ops::Bound::Included(modified_key(user_id, since)), std::ops::Bound::Excluded(format!("{user_id}0")));
            let mut items = Vec::new();
            for r in ctx.db.news_items().modified_key().filter(crate::bounds(&range)) {
                if matches(&r) {
                    items.push(detail(ctx, &r));
                }
                if items.len() >= 5000 {
                    break;
                }
            }
            Ok(json!({ "items": items }))
        }
        "item" => {
            keys(p, &["id"])?;
            let id = int(p.get("id"), 1.0, 4_294_967_295.0)?;
            let Some(r) = ctx.db.news_items().id().find(id).filter(|r| r.owner_id == user_id) else { return fail("notFound") };
            Ok(detail(ctx, &r))
        }
        "feedInfo" => {
            keys(p, &["id"])?;
            let r = feed(p.get("id"))?;
            let all = feed_items(ctx, r.id);
            let recent: Vec<&NewsItem> = all.iter().filter(|i| i.fetched_at >= at - 14.0 * 86_400_000.0).collect();
            let per_day: Vec<usize> = (0..14).map(|day| recent.iter().filter(|i| ((at - i.fetched_at) / 86_400_000.0).floor() as i64 == 13 - day).count()).collect();
            let mut v = expose_feed(&r, &unread_counts(ctx, user_id));
            v["perDay"] = json!(per_day);
            v["starred"] = json!(all.iter().filter(|i| i.starred).count());
            Ok(v)
        }
        "exportData" => {
            keys(p, &["starred"])?;
            if p.get("starred") == Some(&json!(true)) {
                let mut starred: Vec<NewsItem> = ctx.db.news_items().starred_key().filter(user_id).collect();
                starred.sort_by(|a, b| b.id.cmp(&a.id));
                let items: Vec<Value> = starred
                    .iter()
                    .take(5000)
                    .map(|r| {
                        let mut v = detail(ctx, r);
                        v["feed"] = json!(ctx.db.news_feeds().id().find(r.feed_id).map(|f| f.title).unwrap_or_default());
                        v
                    })
                    .collect();
                return Ok(json!({ "items": items }));
            }
            Ok(json!({
                "folders": ctx.db.news_folders().owner_id().filter(user_id).map(|f| json!({ "id": f.id, "name": f.name })).collect::<Vec<_>>(),
                "feeds": feeds().iter().map(|f| json!({ "id": f.id, "url": f.url, "title": f.title, "link": f.link, "folderId": f.folder_id })).collect::<Vec<_>>(),
            }))
        }
        "createFolder" => {
            keys(p, &["name"])?;
            let value = name(p.get("name"))?;
            let existing: Vec<NewsFolder> = ctx.db.news_folders().owner_id().filter(user_id).collect();
            if existing.iter().any(|f| lower(&f.name) == lower(&value)) {
                return fail("alreadyExists");
            }
            if existing.len() >= FOLDERS {
                return fail("quotaExceeded");
            }
            let created = NewsFolder { id: next_id(ctx, "folder")?, owner_id: user_id.into(), name: value, opened: true, created_at: at };
            ctx.db.news_folders().insert(created.clone());
            Ok(json!({ "id": created.id, "name": created.name, "opened": true }))
        }
        "renameFolder" => {
            keys(p, &["id", "name"])?;
            let f = folder(p.get("id"))?;
            let value = name(p.get("name"))?;
            if ctx.db.news_folders().owner_id().filter(user_id).any(|o| o.id != f.id && lower(&o.name) == lower(&value)) {
                return fail("alreadyExists");
            }
            ctx.db.news_folders().id().update(NewsFolder { name: value.clone(), ..f.clone() });
            Ok(json!({ "id": f.id, "name": value, "opened": f.opened }))
        }
        "openFolder" => {
            keys(p, &["id", "opened"])?;
            let f = folder(p.get("id"))?;
            let Some(opened) = p.get("opened").and_then(Value::as_bool) else { return fail("invalidInput") };
            ctx.db.news_folders().id().update(NewsFolder { opened, ..f.clone() });
            Ok(json!({ "id": f.id, "name": f.name, "opened": opened }))
        }
        "deleteFolder" => {
            keys(p, &["id"])?;
            let f = folder(p.get("id"))?;
            for child in feeds().into_iter().filter(|c| c.folder_id == f.id) {
                delete_feed_rows(ctx, &child);
            }
            ctx.db.news_folders().id().delete(f.id);
            Ok(json!({ "id": f.id, "deleted": true }))
        }
        "subscribe" => {
            keys(p, &["url", "folderId", "title", "link", "items", "etag", "lastModifiedHeader", "fullText"])?;
            let feed_url = url(p.get("url"))?;
            let parent = folder_id(p.get("folderId"))?;
            let url_key = format!("{user_id}\n{feed_url}");
            if ctx.db.news_feeds().url_key().find(url_key.clone()).is_some() {
                return fail("alreadyExists");
            }
            if feeds().len() >= FEEDS {
                return fail("quotaExceeded");
            }
            let (refresh_ms, _) = news_preferences(ctx, user_id);
            let link = match p.get("link") {
                Some(v) if v.as_str().is_some_and(|s| !s.is_empty()) || v.as_bool() == Some(true) || v.as_f64().is_some_and(|f| f != 0.0) => optional_url(Some(v))?,
                _ => String::new(),
            };
            let created = NewsFeed {
                id: next_id(ctx, "feed")?,
                owner_id: user_id.into(),
                folder_id: parent,
                url: feed_url,
                url_key,
                title: text(p.get("title"), TITLE, false)?.trim().to_owned(),
                link,
                added: at,
                ordering: 0,
                pinned: false,
                full_text: p.get("fullText") == Some(&json!(true)),
                paused: false,
                etag: text(Some(p.get("etag").unwrap_or(&json!(""))), 500, true)?,
                last_modified_header: text(Some(p.get("lastModifiedHeader").unwrap_or(&json!(""))), 200, true)?,
                last_updated: at,
                next_update: at + refresh_ms,
                due_key: due_key(at + refresh_ms),
                error_count: 0,
                last_error: String::new(),
            };
            ctx.db.news_feeds().insert(created.clone());
            store_entries(ctx, &created, p.get("items").unwrap_or(&json!([])), None)?;
            Ok(expose_feed(&created, &unread_counts(ctx, user_id)))
        }
        "importOpml" => {
            // Feeds are created pending; the fetcher fills titles and items on its next pass.
            keys(p, &["feeds"])?;
            let Some(list) = p.get("feeds").and_then(Value::as_array).filter(|l| l.len() <= FEEDS) else { return fail("invalidInput") };
            let (mut added, mut skipped, mut folders) = (0, 0, 0);
            for value in list {
                let empty = Map::new();
                let input = value.as_object().unwrap_or(&empty);
                keys(input, &["url", "title", "folder"])?;
                let feed_url = url(input.get("url"))?;
                let url_key = format!("{user_id}\n{feed_url}");
                if ctx.db.news_feeds().url_key().find(url_key.clone()).is_some() {
                    skipped += 1;
                    continue;
                }
                if feeds().len() >= FEEDS {
                    return fail("quotaExceeded");
                }
                let mut parent = 0;
                if input.get("folder").is_some_and(|f| f.as_str().is_some_and(|s| !s.is_empty()) || !f.is_null() && !f.is_string() && f != &json!(false) && f != &json!(0)) {
                    let label = name(input.get("folder"))?;
                    let existing: Vec<NewsFolder> = ctx.db.news_folders().owner_id().filter(user_id).collect();
                    if let Some(found) = existing.iter().find(|f| lower(&f.name) == lower(&label)) {
                        parent = found.id;
                    } else {
                        if existing.len() >= FOLDERS {
                            return fail("quotaExceeded");
                        }
                        parent = next_id(ctx, "folder")?;
                        folders += 1;
                        ctx.db.news_folders().insert(NewsFolder { id: parent, owner_id: user_id.into(), name: label, opened: true, created_at: at });
                    }
                }
                let title_value = match input.get("title") {
                    Some(t) if t.as_str().is_some_and(|s| !s.is_empty()) => t.clone(),
                    _ => json!(feed_url),
                };
                ctx.db.news_feeds().insert(NewsFeed {
                    id: next_id(ctx, "feed")?,
                    owner_id: user_id.into(),
                    folder_id: parent,
                    url: feed_url,
                    url_key,
                    title: text(Some(&title_value), TITLE, false)?.trim().to_owned(),
                    link: String::new(),
                    added: at,
                    ordering: 0,
                    pinned: false,
                    full_text: false,
                    paused: false,
                    etag: String::new(),
                    last_modified_header: String::new(),
                    last_updated: 0.0,
                    next_update: 0.0,
                    due_key: due_key(0.0),
                    error_count: 0,
                    last_error: String::new(),
                });
                added += 1;
            }
            Ok(json!({ "added": added, "skipped": skipped, "folders": folders }))
        }
        "updateFeed" => {
            keys(p, &["id", "title", "folderId", "fullText", "pinned", "paused", "ordering", "updateNow"])?;
            let old = feed(p.get("id"))?;
            let mut next = old.clone();
            if p.contains_key("title") {
                next.title = text(p.get("title"), TITLE, false)?.trim().to_owned();
            }
            if p.contains_key("folderId") {
                next.folder_id = folder_id(p.get("folderId"))?;
            }
            for (k, slot) in [("fullText", 0), ("pinned", 1), ("paused", 2)] {
                if let Some(v) = p.get(k) {
                    let Some(b) = v.as_bool() else { return fail("invalidInput") };
                    match slot {
                        0 => next.full_text = b,
                        1 => next.pinned = b,
                        _ => next.paused = b,
                    }
                }
            }
            if p.contains_key("ordering") {
                next.ordering = int(p.get("ordering"), 0.0, 2.0)?;
            }
            if p.get("paused") == Some(&json!(false)) && old.paused || p.get("updateNow") == Some(&json!(true)) {
                next.next_update = 0.0;
            }
            if p.get("paused") == Some(&json!(true)) {
                next.next_update = PAUSED;
            }
            next.due_key = due_key(next.next_update);
            ctx.db.news_feeds().id().update(next.clone());
            Ok(expose_feed(&next, &unread_counts(ctx, user_id)))
        }
        "deleteFeed" => {
            keys(p, &["id"])?;
            let f = feed(p.get("id"))?;
            delete_feed_rows(ctx, &f);
            Ok(json!({ "id": f.id, "deleted": true }))
        }
        "markRead" => {
            // Marks only items no newer than the newest one the reader had loaded.
            keys(p, &["type", "id", "newestItemId"])?;
            let matches = in_scope(p)?;
            let newest = int(p.get("newestItemId"), 0.0, 4_294_967_295.0)?;
            let mut ids = Vec::new();
            for r in ctx.db.news_items().unread_key().filter(user_id).collect::<Vec<_>>() {
                if r.id <= newest && matches(&r) {
                    set_state(ctx, &r, Some(false), None, None);
                    ids.push(r.id);
                }
            }
            Ok(json!({ "changed": ids.len(), "ids": ids.iter().take(5000).collect::<Vec<_>>() }))
        }
        "setItems" => {
            keys(p, &["ids", "unread", "starred"])?;
            let Some(ids) = p.get("ids").and_then(Value::as_array).filter(|a| a.len() <= 1000) else { return fail("invalidInput") };
            let flag = |k: &str| -> Res<Option<bool>> {
                match p.get(k) {
                    None => Ok(None),
                    Some(Value::Bool(b)) => Ok(Some(*b)),
                    _ => fail("invalidInput"),
                }
            };
            let (unread, starred) = (flag("unread")?, flag("starred")?);
            let mut items = Vec::new();
            for value in ids {
                let id = int(Some(value), 1.0, 4_294_967_295.0)?;
                let Some(r) = ctx.db.news_items().id().find(id).filter(|r| r.owner_id == user_id) else { continue };
                items.push(expose_item(&set_state(ctx, &r, unread, starred, None)));
            }
            Ok(json!({ "items": items }))
        }
        "starByGuid" => {
            keys(p, &["items", "starred"])?;
            let (Some(list), Some(starred)) = (p.get("items").and_then(Value::as_array).filter(|a| a.len() <= 1000), p.get("starred").and_then(Value::as_bool)) else { return fail("invalidInput") };
            let mut changed = 0;
            for value in list {
                let feed_id = int(value.get("feedId"), 1.0, 4_294_967_295.0)?;
                let Some(parent) = ctx.db.news_feeds().id().find(feed_id).filter(|f| f.owner_id == user_id) else { continue };
                if let Some(r) = ctx.db.news_items().dedupe_key().find(format!("{}:{}", parent.id, crate::text_of(value.get("guidHash")))) {
                    set_state(ctx, &r, None, Some(starred), None);
                    changed += 1;
                }
            }
            Ok(json!({ "changed": changed }))
        }
        "setPosition" => {
            keys(p, &["id", "position"])?;
            let id = int(p.get("id"), 1.0, 4_294_967_295.0)?;
            let Some(r) = ctx.db.news_items().id().find(id).filter(|r| r.owner_id == user_id) else { return fail("notFound") };
            let Some(position) = p.get("position").and_then(Value::as_f64).filter(|f| f.is_finite() && *f >= 0.0 && *f < 1e7) else { return fail("invalidInput") };
            Ok(expose_item(&set_state(ctx, &r, None, None, Some(js_round(position)))))
        }
        _ => fail("invalidInput"),
    }
}

/// `Math.round`: halves round up.
pub fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// Service-only fetcher steps: claim due feeds, then store results or failures.
pub fn news_worker(ctx: &ReducerContext, operation: &str, p: &Doc) -> Res<Value> {
    let at = now(ctx);
    if operation == "due" {
        keys(p, &["limit"])?;
        if !app_enabled(ctx, "news") {
            return Ok(json!({ "feeds": [] }));
        }
        let limit = int(p.get("limit"), 1.0, 50.0)? as usize;
        let range = (std::ops::Bound::Included(due_key(0.0)), std::ops::Bound::Included(due_key(at)));
        let mut due = Vec::new();
        for r in ctx.db.news_feeds().due_key().filter(crate::bounds(&range)).collect::<Vec<_>>() {
            if r.paused || ctx.db.accounts().id().find(r.owner_id.clone()).is_none_or(|a| !a.enabled) {
                continue;
            }
            // A lease: if the runtime stops mid-fetch the feed becomes due again in ten minutes.
            ctx.db.news_feeds().id().update(NewsFeed { next_update: at + 600_000.0, due_key: due_key(at + 600_000.0), ..r.clone() });
            due.push(json!({ "id": r.id, "url": r.url, "etag": r.etag, "lastModifiedHeader": r.last_modified_header, "fullText": r.full_text,
                "known": feed_items(ctx, r.id).iter().map(|i| i.guid_hash.clone()).collect::<Vec<_>>() }));
            if due.len() >= limit {
                break;
            }
        }
        return Ok(json!({ "feeds": due }));
    }
    let id = int(p.get("feedId"), 1.0, 4_294_967_295.0)?;
    let Some(r) = ctx.db.news_feeds().id().find(id) else { return Ok(json!({ "stored": false })) };
    let (refresh_ms, _) = news_preferences(ctx, &r.owner_id);
    match operation {
        "store" => {
            keys(p, &["feedId", "etag", "lastModifiedHeader", "title", "link", "items", "notModified", "present"])?;
            let next_at = if r.paused { PAUSED } else { at + refresh_ms };
            let mut next = NewsFeed {
                etag: text(Some(p.get("etag").unwrap_or(&json!(""))), 500, true)?,
                last_modified_header: text(Some(p.get("lastModifiedHeader").unwrap_or(&json!(""))), 200, true)?,
                error_count: 0,
                last_error: String::new(),
                last_updated: at,
                next_update: next_at,
                due_key: due_key(next_at),
                ..r.clone()
            };
            // Imported feeds take their title from the first successful fetch.
            if p.get("title").and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty()) && (r.last_updated == 0.0 || r.title == r.url) {
                next.title = text(p.get("title"), TITLE, false)?.trim().to_owned();
            }
            if p.get("link").and_then(Value::as_str).is_some_and(|l| !l.is_empty()) {
                next.link = optional_url(p.get("link"))?;
            }
            ctx.db.news_feeds().id().update(next.clone());
            let added = if p.get("notModified") == Some(&json!(true)) { 0 } else { store_entries(ctx, &next, p.get("items").unwrap_or(&json!([])), p.get("present"))? };
            Ok(json!({ "stored": true, "added": added }))
        }
        "fail" => {
            keys(p, &["feedId", "error"])?;
            let errors = r.error_count + 1;
            let delay = (refresh_ms * 2f64.powi((errors - 1).min(6) as i32)).min(86_400_000.0);
            let next_at = if r.paused { PAUSED } else { at + delay };
            ctx.db.news_feeds().id().update(NewsFeed { error_count: errors, last_error: text(p.get("error"), 500, false)?, next_update: next_at, due_key: due_key(next_at), ..r });
            Ok(json!({ "stored": true }))
        }
        _ => fail("invalidInput"),
    }
}

pub fn delete_news_account(ctx: &ReducerContext, owner_id: &str) {
    for f in ctx.db.news_feeds().owner_id().filter(owner_id).collect::<Vec<_>>() {
        delete_feed_rows(ctx, &f);
    }
    for f in ctx.db.news_folders().owner_id().filter(owner_id).collect::<Vec<_>>() {
        ctx.db.news_folders().id().delete(f.id);
    }
    ctx.db.news_counts().owner_id().delete(owner_id.to_owned());
}

pub fn news_search(ctx: &ReducerContext, user_id: &str, s: &mut crate::global_search::Search) {
    let feeds: Vec<NewsFeed> = ctx.db.news_feeds().owner_id().filter(user_id).collect();
    let titles: HashMap<u32, String> = feeds.iter().map(|f| (f.id, f.title.clone())).collect();
    for f in &feeds {
        if !s.visit() {
            return;
        }
        if s.matches(&f.title) {
            s.found("News", format!("feed-{}", f.id), f.title.clone(), "News feed".into(), format!("/news?feed={}", f.id), String::new());
        }
    }
    let range = prefix(user_id);
    for r in ctx.db.news_items().owner_key().filter(crate::bounds(&range)) {
        if !s.visit() {
            return;
        }
        if s.matches(&r.title) {
            let title = if r.title.is_empty() { "Untitled".to_owned() } else { r.title.clone() };
            s.found("News", format!("item-{}", r.id), title, titles.get(&r.feed_id).cloned().unwrap_or_else(|| "News".into()), format!("/news?item={}", r.id), String::new());
        }
    }
}

pub fn news_snapshot(ctx: &ReducerContext) -> Value {
    let strip = |mut v: Value, keys: &[&str]| {
        if let Some(o) = v.as_object_mut() {
            for k in keys {
                o.remove(*k);
            }
        }
        v
    };
    json!({
        "folders": ctx.db.news_folders().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "feeds": ctx.db.news_feeds().iter().map(|r| strip(row(&r), &["urlKey", "dueKey"])).collect::<Vec<_>>(),
        "items": ctx.db.news_items().iter().map(|r| {
            let mut v = strip(row(&r), &["ownerKey", "feedKey", "unreadKey", "starredKey", "modifiedKey", "dedupeKey"]);
            v["body"] = json!(ctx.db.news_bodies().item_id().find(r.id).map(|b| b.body).unwrap_or_default());
            v
        }).collect::<Vec<_>>(),
        "sequences": ctx.db.news_sequences().iter().map(|r| row(&r)).collect::<Vec<_>>(),
    })
}

/// V7 backups carry News; earlier formats have none. Keys and counters are rebuilt.
pub fn restore_news(ctx: &ReducerContext, value: Option<&Value>) -> Res<()> {
    let Some(value) = value else { return Ok(()) };
    for k in ["folders", "feeds", "items", "sequences"] {
        if !value[k].is_array() {
            return fail("invalidInput");
        }
    }
    let mut highest: HashMap<&str, u32> = HashMap::new();
    for v in value["folders"].as_array().expect("array") {
        let r: NewsFolder = serde_json::from_value(v.clone()).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.accounts().id().find(r.owner_id.clone()).is_none() || r.id < 1 || text(Some(&json!(r.name)), 200, false)?.trim() != r.name {
            return fail("invalidInput");
        }
        *highest.entry("folder").or_insert(0) = highest.get("folder").copied().unwrap_or(0).max(r.id);
        ctx.db.news_folders().insert(r);
    }
    for v in value["feeds"].as_array().expect("array") {
        let mut v = v.clone();
        v["urlKey"] = json!("");
        v["dueKey"] = json!("");
        let r: NewsFeed = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.accounts().id().find(r.owner_id.clone()).is_none() || r.id < 1 || r.folder_id != 0 && ctx.db.news_folders().id().find(r.folder_id).is_none_or(|f| f.owner_id != r.owner_id) {
            return fail("invalidInput");
        }
        url(Some(&json!(r.url)))?;
        *highest.entry("feed").or_insert(0) = highest.get("feed").copied().unwrap_or(0).max(r.id);
        let url_key = format!("{}\n{}", r.owner_id, r.url);
        let due = due_key(r.next_update);
        ctx.db.news_feeds().insert(NewsFeed { url_key, due_key: due, ..r });
    }
    for v in value["items"].as_array().expect("array") {
        let body = text(v.get("body"), BODY, true)?;
        let mut v = v.clone();
        for k in ["dedupeKey", "ownerKey", "feedKey", "unreadKey", "starredKey", "modifiedKey"] {
            v[k] = json!("");
        }
        let r: NewsItem = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.news_feeds().id().find(r.feed_id).is_none_or(|f| f.owner_id != r.owner_id) || r.id < 1 {
            return fail("invalidInput");
        }
        *highest.entry("item").or_insert(0) = highest.get("item").copied().unwrap_or(0).max(r.id);
        let id = r.id;
        let owner = r.owner_id.clone();
        ctx.db.news_items().insert(NewsItem {
            dedupe_key: format!("{}:{}", r.feed_id, r.guid_hash),
            owner_key: format!("{}/{}", r.owner_id, news_key(r.id)),
            feed_key: format!("{}/{}", feed_prefix(r.feed_id), news_key(r.id)),
            unread_key: if r.unread { r.owner_id.clone() } else { String::new() },
            starred_key: if r.starred { r.owner_id.clone() } else { String::new() },
            modified_key: modified_key(&r.owner_id, r.last_modified),
            ..r
        });
        ctx.db.news_bodies().insert(NewsBody { item_id: id, body });
        adjust_count(ctx, &owner, 1);
    }
    let saved: Vec<NewsSequence> = serde_json::from_value(value["sequences"].clone()).map_err(|_| "invalidInput".to_owned())?;
    for name in ["folder", "feed", "item"] {
        let previous = saved.iter().find(|s| s.name == name).map_or(0, |s| s.value);
        ctx.db.news_sequences().insert(NewsSequence { name: name.into(), value: previous.max(highest.get(name).copied().unwrap_or(0)) });
    }
    Ok(())
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_news(ctx: &mut spacetimedb::ProcedureContext, sessionId: String, operation: String, document: String) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let user = crate::from_session(tx, &sessionId)?;
        if let Some(password) = tx.db.app_passwords().session_id().find(sessionId.clone()) {
            let permission = if READ_OPERATIONS.contains(&operation.as_str()) { "read" } else { "write" };
            if password.revoked || password.expires_at <= now(tx) || !crate::scopes::password_allows(&password.scope, "news", permission) {
                return fail("forbidden");
            }
        }
        if document.len() > 24 * 1024 * 1024 {
            return fail("operationTooLarge");
        }
        news_operation(tx, &user.id, &operation, &crate::document(&document)?)
    })
}

#[spacetimedb::procedure]
pub fn service_news_worker(ctx: &mut spacetimedb::ProcedureContext, operation: String, document: String) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        if document.len() > 24 * 1024 * 1024 {
            return fail("operationTooLarge");
        }
        news_worker(tx, &operation, &crate::document(&document)?)
    })
}
