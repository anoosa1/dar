//! News network effects: background refresh loop, discovery, subscribe, full-text extraction,
//! OPML import/export and the image proxy (`next/runtime/news/runtime.ts`).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{self, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::db::Database;
use crate::error::{AppError, Result};
use crate::net::{FetchOptions, FetchResult, safe_fetch};
use crate::news::feed::{NewsEntry, ParsedFeed, decode_body, parse_feed};
use crate::news::html::{discover_feeds, extract_article, text_of, trunc_utf16};
use crate::news::opml::{export_opml, parse_opml};

const FEED_ACCEPT: &str = "application/rss+xml, application/atom+xml, application/feed+json, application/json;q=0.9, application/xml;q=0.9, text/xml;q=0.9, text/html;q=0.5, */*;q=0.1";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Due {
    id: u32,
    url: String,
    #[serde(default)]
    etag: String,
    #[serde(default)]
    last_modified_header: String,
    #[serde(default)]
    full_text: bool,
    #[serde(default)]
    known: Vec<String>,
}

struct TickState {
    active: bool,
    generation: u64,
}

struct Inner {
    db: Database,
    allow_private: bool,
    closed: watch::Sender<bool>,
    done: watch::Sender<u64>,
    state: Mutex<TickState>,
    bg_task: Mutex<Option<JoinHandle<()>>>,
}

/// Owns all News network effects: the background refresh loop, discovery, subscribe,
/// full-text extraction and the image proxy. Closing stops the loop and aborts fetches.
#[derive(Clone)]
pub struct NewsRuntime {
    inner: Arc<Inner>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64
}

/// Splits entries so each database call stays well under the request limits.
fn chunks(entries: &[NewsEntry], max: usize) -> Vec<Vec<NewsEntry>> {
    let mut out: Vec<Vec<NewsEntry>> = vec![Vec::new()];
    let mut size = 0usize;
    for entry in entries {
        let bytes = serde_json::to_string(entry).map_or(0, |s| s.len());
        if size + bytes > max && !out.last().expect("chunk").is_empty() {
            out.push(Vec::new());
            size = 0;
        }
        out.last_mut().expect("chunk").push(entry.clone());
        size += bytes;
    }
    out
}

async fn wait_closed(rx: &mut watch::Receiver<bool>) {
    if *rx.borrow_and_update() {
        return;
    }
    while rx.changed().await.is_ok() {
        if *rx.borrow_and_update() {
            return;
        }
    }
}

fn summary(parsed: &ParsedFeed, feed_url: &str) -> Value {
    let mut sorted = parsed.entries.clone();
    sorted.sort_by(|a, b| b.pub_date.cmp(&a.pub_date));
    let items: Vec<Value> = sorted
        .into_iter()
        .take(3)
        .map(|e| json!({ "title": e.title, "pubDate": e.pub_date }))
        .collect();
    json!({ "url": feed_url, "title": parsed.title, "kind": parsed.kind, "items": items })
}

impl NewsRuntime {
    pub fn new(db: Database, allow_private: bool) -> Self {
        let (closed, _) = watch::channel(false);
        let (done, _) = watch::channel(0u64);
        let runtime = Self {
            inner: Arc::new(Inner {
                db,
                allow_private,
                closed,
                done,
                state: Mutex::new(TickState {
                    active: false,
                    generation: 0,
                }),
                bg_task: Mutex::new(None),
            }),
        };
        let ticker = runtime.clone();
        let handle = tokio::spawn(async move {
            let mut rx = ticker.inner.closed.subscribe();
            tokio::select! {
                biased;
                _ = wait_closed(&mut rx) => return,
                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
            }
            loop {
                if ticker.is_closing() {
                    break;
                }
                ticker.poke();
                tokio::select! {
                    biased;
                    _ = wait_closed(&mut rx) => break,
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
            }
        });
        *runtime.inner.bg_task.lock().expect("bg_task") = Some(handle);
        runtime
    }

    fn is_closing(&self) -> bool {
        *self.inner.closed.borrow()
    }

    async fn call(&self, session_id: &str, operation: &str, value: Value) -> Result<Value> {
        self.inner
            .db
            .call(
                "service_news",
                json!([session_id, operation, value.to_string()]),
            )
            .await
    }

    async fn worker(&self, operation: &str, value: Value) -> Result<Value> {
        self.inner
            .db
            .call("service_news_worker", json!([operation, value.to_string()]))
            .await
    }

    async fn fetch(&self, url: &str, options: &FetchOptions<'_>) -> Result<FetchResult> {
        let mut rx = self.inner.closed.subscribe();
        tokio::select! {
            biased;
            _ = wait_closed(&mut rx) => Err(AppError::new("cancelled", 499)),
            res = safe_fetch(url, options) => res,
        }
    }

    async fn load(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<(FetchResult, Option<ParsedFeed>)> {
        let response = self
            .fetch(
                url,
                &FetchOptions {
                    accept: FEED_ACCEPT,
                    etag,
                    last_modified,
                    allow_private: self.inner.allow_private,
                    ..Default::default()
                },
            )
            .await?;
        if response.status == 304 {
            return Ok((response, None));
        }
        if response.status == 410 {
            return Err(AppError::with(
                "feedGone",
                502,
                "The feed was removed by its site (410 Gone).",
            ));
        }
        if !(200..300).contains(&response.status) {
            return Err(AppError::with(
                "feedUnavailable",
                502,
                &format!("The site answered {}.", response.status),
            ));
        }
        let feed = parse_feed(
            &response.body,
            &response.content_type,
            &response.url,
            now_ms(),
        )?;
        Ok((response, Some(feed)))
    }

    /// Fetches a single article's web page on demand and returns its sanitized main prose.
    pub async fn extract_url(&self, url: &str) -> Result<String> {
        let trimmed = url.trim();
        let lower = trimmed.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err(AppError::with(
                "invalidInput",
                400,
                "This article has no web address to fetch.",
            ));
        }
        let page = self
            .fetch(
                trimmed,
                &FetchOptions {
                    accept: "text/html,application/xhtml+xml",
                    etag: None,
                    last_modified: None,
                    max_bytes: 3 * 1024 * 1024,
                    timeout: Duration::from_secs(15),
                    allow_private: self.inner.allow_private,
                },
            )
            .await?;
        if page.status != 200 || !page.content_type.contains("html") {
            return Err(AppError::with(
                "fullTextUnavailable",
                502,
                "The original page did not return readable HTML.",
            ));
        }
        let body = extract_article(&decode_body(&page.body, &page.content_type), &page.url);
        if body.is_empty() {
            return Err(AppError::with(
                "fullTextUnavailable",
                422,
                "Could not extract full article text from this page.",
            ));
        }
        Ok(body)
    }

    /// Replaces summaries with the article page's main text for entries not stored yet.
    async fn full_text(&self, entries: &mut [NewsEntry], known: &HashSet<String>) {
        let targets: Vec<(usize, String)> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.url.is_empty() && !known.contains(&e.guid_hash))
            .take(15)
            .map(|(i, e)| (i, e.url.clone()))
            .collect();
        let results: Vec<Option<(usize, String)>> = stream::iter(targets)
            .map(|(idx, url)| async move {
                let page = self
                    .fetch(
                        &url,
                        &FetchOptions {
                            accept: "text/html,application/xhtml+xml",
                            etag: None,
                            last_modified: None,
                            max_bytes: 3 * 1024 * 1024,
                            timeout: Duration::from_secs(15),
                            allow_private: self.inner.allow_private,
                        },
                    )
                    .await
                    .ok()?;
                if page.status != 200 || !page.content_type.contains("html") {
                    return None;
                }
                let body = extract_article(&decode_body(&page.body, &page.content_type), &page.url);
                (!body.is_empty()).then_some((idx, body))
            })
            .buffer_unordered(3)
            .collect()
            .await;
        for (idx, body) in results.into_iter().flatten() {
            entries[idx].excerpt = trunc_utf16(&text_of(&body, 2000), 300).to_owned();
            entries[idx].body = body;
        }
    }

    async fn refresh(&self, feed: Due) {
        let outcome: Result<()> = async {
            let (response, parsed) = self.load(&feed.url, Some(&feed.etag), Some(&feed.last_modified_header)).await?;
            let etag = trunc_utf16(&response.etag, 500);
            let last_mod = trunc_utf16(&response.last_modified, 200);
            let Some(mut parsed) = parsed else {
                self.worker("store", json!({ "feedId": feed.id, "etag": etag, "lastModifiedHeader": last_mod, "notModified": true })).await?;
                return Ok(());
            };
            if feed.full_text {
                let known: HashSet<String> = feed.known.into_iter().collect();
                self.full_text(&mut parsed.entries, &known).await;
            }
            let present: Vec<String> = parsed.entries.iter().map(|e| e.guid_hash.clone()).collect();
            let parts = chunks(&parsed.entries, 900 * 1024);
            let multi = parts.len() > 1;
            for part in parts {
                let mut payload = json!({
                    "feedId": feed.id,
                    "etag": etag,
                    "lastModifiedHeader": last_mod,
                    "title": parsed.title,
                    "link": parsed.link,
                    "items": part,
                });
                if multi {
                    payload["present"] = json!(present);
                }
                self.worker("store", payload).await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            if self.is_closing() {
                return;
            }
            let message = if error.message.trim().is_empty() {
                "The feed could not be updated."
            } else {
                error.message.as_str()
            };
            let _ = self
                .worker(
                    "fail",
                    json!({ "feedId": feed.id, "error": trunc_utf16(message, 500) }),
                )
                .await;
        }
    }

    fn start_or_join_tick(&self) -> Option<u64> {
        if self.is_closing() {
            return None;
        }
        let mut state = self.inner.state.lock().expect("tick state");
        if state.active {
            return Some(state.generation);
        }
        state.active = true;
        state.generation += 1;
        let generation = state.generation;
        drop(state);
        let rt = self.clone();
        tokio::spawn(async move {
            rt.run_tick(generation).await;
        });
        Some(generation)
    }

    async fn run_tick(&self, generation: u64) {
        for _ in 0..20 {
            if self.is_closing() {
                break;
            }
            let Ok(value) = self.worker("due", json!({ "limit": 20 })).await else {
                break;
            };
            let feeds: Vec<Due> = value["feeds"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            if feeds.is_empty() {
                break;
            }
            stream::iter(feeds)
                .map(|feed| self.refresh(feed))
                .buffer_unordered(4)
                .collect::<Vec<()>>()
                .await;
        }
        {
            let mut state = self.inner.state.lock().expect("tick state");
            state.active = false;
        }
        let _ = self.inner.done.send(generation);
    }

    /// Triggers a refresh pass if none is running, without waiting.
    pub fn poke(&self) {
        let _ = self.start_or_join_tick();
    }

    /// Runs a refresh pass (or joins the active one) and waits for it to finish.
    pub async fn tick(&self) {
        let Some(target) = self.start_or_join_tick() else {
            return;
        };
        let mut done_rx = self.inner.done.subscribe();
        let mut closed_rx = self.inner.closed.subscribe();
        loop {
            if *done_rx.borrow_and_update() >= target || *closed_rx.borrow_and_update() {
                return;
            }
            tokio::select! {
                biased;
                _ = wait_closed(&mut closed_rx) => return,
                res = done_rx.changed() => {
                    if res.is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// A feed address is used directly; a web page is searched for advertised feeds.
    pub async fn discover(&self, input: &str) -> Result<Vec<Value>> {
        let trimmed = input.trim();
        let lower = trimmed.to_ascii_lowercase();
        let url = if lower.starts_with("http://") || lower.starts_with("https://") {
            trimmed.to_owned()
        } else {
            format!("https://{trimmed}")
        };
        let page = self
            .fetch(
                &url,
                &FetchOptions {
                    accept: FEED_ACCEPT,
                    allow_private: self.inner.allow_private,
                    ..Default::default()
                },
            )
            .await?;
        if !(200..300).contains(&page.status) {
            return Err(AppError::with(
                "feedUnavailable",
                502,
                &format!("The site answered {}.", page.status),
            ));
        }
        match parse_feed(&page.body, &page.content_type, &page.url, now_ms()) {
            Ok(parsed) => return Ok(vec![summary(&parsed, &page.url)]),
            Err(error) if error.code != "notAFeed" => return Err(error),
            Err(_) => {}
        }
        let mut candidates: Vec<String> =
            discover_feeds(&decode_body(&page.body, &page.content_type), &page.url)
                .into_iter()
                .map(|c| c.url)
                .collect();
        if candidates.is_empty()
            && let Ok(base) = url::Url::parse(&page.url)
        {
            candidates = [
                "/feed",
                "/rss",
                "/feed.xml",
                "/atom.xml",
                "/rss.xml",
                "/index.xml",
            ]
            .into_iter()
            .filter_map(|p| base.join(p).ok().map(|u| u.to_string()))
            .collect();
        }
        let probed: Vec<Option<(usize, String, Value)>> =
            stream::iter(candidates.into_iter().take(8).enumerate())
                .map(|(idx, candidate)| async move {
                    let (_, feed) = self.load(&candidate, None, None).await.ok()?;
                    let feed = feed?;
                    let item = summary(&feed, &candidate);
                    Some((idx, candidate, item))
                })
                .buffer_unordered(3)
                .collect()
                .await;
        let mut found: Vec<(usize, String, Value)> = probed.into_iter().flatten().collect();
        found.sort_by_key(|(idx, _, _)| *idx);
        let mut seen = HashSet::new();
        let unique: Vec<Value> = found
            .into_iter()
            .filter(|(_, u, _)| seen.insert(u.clone()))
            .map(|(_, _, v)| v)
            .collect();
        if unique.is_empty() {
            return Err(AppError::with(
                "notAFeed",
                422,
                "No RSS, Atom or JSON feed was found at that address.",
            ));
        }
        Ok(unique)
    }

    pub async fn subscribe(
        &self,
        session_id: &str,
        url: &str,
        folder_id: i64,
        title: Option<&str>,
        full_text: bool,
    ) -> Result<Value> {
        let (response, feed) = self.load(url, None, None).await?;
        let mut feed = feed
            .ok_or_else(|| AppError::with("feedUnavailable", 502, "The feed could not be read."))?;
        if full_text {
            self.full_text(&mut feed.entries, &HashSet::new()).await;
        }
        let parts = chunks(&feed.entries, 900 * 1024);
        let present: Vec<String> = feed.entries.iter().map(|e| e.guid_hash.clone()).collect();
        let chosen_title = title
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&feed.title);
        let etag = trunc_utf16(&response.etag, 500);
        let last_mod = trunc_utf16(&response.last_modified, 200);
        let created = self
            .call(
                session_id,
                "subscribe",
                json!({
                    "url": url,
                    "folderId": folder_id,
                    "title": chosen_title,
                    "link": feed.link,
                    "items": parts[0],
                    "etag": etag,
                    "lastModifiedHeader": last_mod,
                    "fullText": full_text,
                }),
            )
            .await?;
        for part in &parts[1..] {
            self.worker(
                "store",
                json!({
                    "feedId": created["id"],
                    "etag": etag,
                    "lastModifiedHeader": last_mod,
                    "items": part,
                    "present": present,
                }),
            )
            .await?;
        }
        if parts.len() > 1 {
            let feeds = self.call(session_id, "feeds", json!({})).await?;
            if let Some(found) = feeds["feeds"]
                .as_array()
                .and_then(|list| list.iter().find(|f| f["id"] == created["id"]))
            {
                return Ok(found.clone());
            }
        }
        Ok(created)
    }

    pub async fn import_opml(&self, session_id: &str, text: &str) -> Result<Value> {
        let feeds = parse_opml(text)?;
        let result = self
            .call(session_id, "importOpml", json!({ "feeds": feeds }))
            .await?;
        self.poke();
        Ok(result)
    }

    pub async fn export_opml(&self, session_id: &str, owner: &str) -> Result<String> {
        let data = self.call(session_id, "exportData", json!({})).await?;
        Ok(export_opml(&data, owner))
    }

    /// Images load through Dar so sites never see the reader; served inert and never as SVG.
    pub async fn image(&self, url: &str) -> Result<Response> {
        let response = self
            .fetch(
                url,
                &FetchOptions {
                    accept: "image/avif,image/webp,image/png,image/jpeg,image/gif;q=0.9",
                    etag: None,
                    last_modified: None,
                    max_bytes: 8 * 1024 * 1024,
                    timeout: Duration::from_secs(15),
                    allow_private: self.inner.allow_private,
                },
            )
            .await?;
        let media_type = response.content_type.split(';').next().unwrap_or("").trim();
        if response.status != 200
            || !matches!(
                media_type,
                "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif"
            )
        {
            return Err(AppError::not_found());
        }
        let len = response.body.len().to_string();
        let mut res = (StatusCode::OK, response.body).into_response();
        let headers = res.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(media_type).map_err(|_| AppError::internal())?,
        );
        headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&len).map_err(|_| AppError::internal())?,
        );
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=86400"),
        );
        headers.insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("inline"),
        );
        Ok(res)
    }

    pub async fn close(&self) {
        let _ = self.inner.closed.send(true);
        let bg = self.inner.bg_task.lock().expect("bg_task").take();
        if let Some(handle) = bg {
            let _ = handle.await;
        }
        let target = {
            let state = self.inner.state.lock().expect("tick state");
            state.active.then_some(state.generation)
        };
        if let Some(target) = target {
            let mut done_rx = self.inner.done.subscribe();
            while *done_rx.borrow_and_update() < target {
                if done_rx.changed().await.is_err() {
                    break;
                }
            }
        }
    }
}
