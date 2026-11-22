//! Background work (`runtime/files/maintenance.ts`, `checksums.ts`, `text-index.ts`,
//! `migrate.ts`, `pim/reminders.ts`, `pim/subscriptions.ts` and the maintenance loop of
//! `server.ts`): storage collection, the orphan sweep, WebDAV checksums, search text, reminder
//! delivery, calendar subscription sync and the one-time move to content-addressed names.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;

use crate::blobs::{Blobs, is_blob_id};
use crate::db::Database;
use crate::digests::{Md5, Sha1, sha1_hex};
use crate::error::{AppError, Result};

const TEXT_INDEX_BYTES: u64 = 2 * 1024 * 1024;
const TEXT_INDEX_CHARS: usize = 256 * 1024;

/// Claims blobs no version or backup pin uses, then removes and acknowledges them.
pub async fn reconcile_storage(db: &Database, blobs: &Blobs) -> Result<usize> {
    let candidates = db.call("service_maintenance", json!([])).await?;
    let mut removed = 0;
    for id in candidates.as_array().into_iter().flatten().filter_map(Value::as_str) {
        if blobs.remove(id).await? {
            db.call("service_acknowledge_gc", json!([id])).await?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Removes stored content the database never recorded, past its grace period.
pub async fn sweep_orphans(db: &Database, blobs: &Blobs) -> Result<usize> {
    let mut removed = 0;
    for page in blobs.settled().await.chunks(1000) {
        let unknown = db.read("service_unknown_blobs", json!([serde_json::to_string(page).expect("ids")])).await?;
        for id in unknown.as_array().into_iter().flatten().filter_map(Value::as_str) {
            if blobs.remove(id).await? {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// A worker that runs a pass when nudged; a nudge during a pass runs one more.
#[derive(Clone)]
pub struct Worker {
    notify: Arc<Notify>,
    stopped: Arc<AtomicBool>,
}

impl Worker {
    fn start<F, Fut>(name: &'static str, pass: F) -> Worker
    where
        F: Fn(Arc<AtomicBool>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        let worker = Worker { notify: Arc::new(Notify::new()), stopped: Arc::new(AtomicBool::new(false)) };
        let (notify, stopped) = (worker.notify.clone(), worker.stopped.clone());
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                if pass(stopped.clone()).await.is_err() {
                    eprintln!("{name} will retry.");
                }
            }
        });
        worker
    }

    pub fn nudge(&self) {
        if !self.stopped.load(Ordering::Relaxed) {
            self.notify.notify_one();
        }
    }

    pub fn close(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.notify.notify_one();
    }
}

async fn checksums_of(blobs: &Blobs, id: &str, stopped: &AtomicBool) -> Result<Option<Value>> {
    let path = blobs.path(id)?;
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(AppError::internal()),
    };
    if !file.metadata().await.map_err(|_| AppError::internal())?.is_file() {
        return Ok(None);
    }
    let (mut sha1, mut md5) = (Sha1::default(), Md5::default());
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        if stopped.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let n = file.read(&mut buffer).await.map_err(|_| AppError::internal())?;
        if n == 0 {
            break;
        }
        sha1.update(&buffer[..n]);
        md5.update(&buffer[..n]);
    }
    Ok(Some(json!({ "id": id, "sha1": sha1.hex(), "md5": md5.hex() })))
}

/// Records the SHA-1 and MD5 of stored content that has none, for WebDAV sync clients.
pub fn checksum_worker(db: Database, blobs: Blobs) -> Worker {
    Worker::start("Content checksums", move |stopped| {
        let (db, blobs) = (db.clone(), blobs.clone());
        async move {
            let mut after = String::new();
            while !stopped.load(Ordering::Relaxed) {
                let ids = db.read("service_blobs_without_checksum", json!([after, 50])).await?;
                let ids: Vec<String> = ids.as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
                if ids.is_empty() {
                    return Ok(());
                }
                let mut items = Vec::new();
                for id in &ids {
                    if let Some(sums) = checksums_of(&blobs, id, &stopped).await? {
                        items.push(sums);
                    }
                }
                if !items.is_empty() && !stopped.load(Ordering::Relaxed) {
                    db.call("service_record_checksums", json!([Value::Array(items).to_string()])).await?;
                }
                after = ids.last().cloned().unwrap_or_default();
            }
            Ok(())
        }
    })
}

async fn text_of(blobs: &Blobs, id: &str) -> Result<Option<Value>> {
    let path = blobs.path(id)?;
    let meta = match tokio::fs::metadata(&path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(AppError::internal()),
    };
    if !meta.is_file() || meta.len() > TEXT_INDEX_BYTES {
        return Ok(None);
    }
    let bytes = tokio::fs::read(&path).await.map_err(|_| AppError::internal())?;
    // Content that is not UTF-8 text is recorded as empty so it is not read again; NUL means binary.
    let text = match String::from_utf8(bytes) {
        Ok(t) if !t.contains('\0') => String::from_utf16_lossy(&t.encode_utf16().take(TEXT_INDEX_CHARS).collect::<Vec<_>>()),
        _ => String::new(),
    };
    Ok(Some(json!({ "id": id, "text": text })))
}

/// Records the text of notes and other text files for the shared search.
pub fn text_index_worker(db: Database, blobs: Blobs) -> Worker {
    Worker::start("Search text indexing", move |stopped| {
        let (db, blobs) = (db.clone(), blobs.clone());
        async move {
            let mut after = String::new();
            while !stopped.load(Ordering::Relaxed) {
                let items = db.read("service_blobs_without_text", json!([after, 50])).await?;
                let items = items.as_array().cloned().unwrap_or_default();
                if items.is_empty() {
                    return Ok(());
                }
                let mut texts = Vec::new();
                for item in &items {
                    if stopped.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    if let Some(value) = text_of(&blobs, item["id"].as_str().unwrap_or_default()).await? {
                        texts.push(value);
                    }
                }
                // Keep each call well under the request limit.
                let mut start = 0;
                while start < texts.len() && !stopped.load(Ordering::Relaxed) {
                    let (mut size, mut end) = (0, start);
                    while end < texts.len() && (end == start || size + texts[end]["text"].as_str().map_or(0, str::len) < 4 * 1024 * 1024) {
                        size += texts[end]["text"].as_str().map_or(0, str::len);
                        end += 1;
                    }
                    db.call("service_record_texts", json!([Value::Array(texts[start..end].to_vec()).to_string()])).await?;
                    start = end;
                }
                after = items.last().and_then(|i| i["id"].as_str()).unwrap_or_default().to_owned();
            }
            Ok(())
        }
    })
}

fn epoch_of(value: &Value) -> i64 {
    value.as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map_or(0, |d| d.timestamp_millis())
}

/// Plans due reminders: each resource catches up from its durable watermark (`deliverReminders`).
pub async fn deliver_reminders(db: &Database) -> Result<()> {
    let work = db.call("service_pim_reminder_work", json!([])).await?;
    for item in work.as_array().into_iter().flatten() {
        let resource = &item["resource"];
        let (from, to) = (item["from"].as_f64().unwrap_or(0.0) as i64, item["to"].as_f64().unwrap_or(0.0) as i64);
        let zone = item["timezone"].as_str().unwrap_or("UTC");
        let content = resource["content"].as_str().unwrap_or_default();
        let planned: Result<Vec<Value>> = (|| {
            let occurrences = if crate::pim::tasks::is_task(content) {
                let task = crate::pim::tasks::read_task(content)?;
                if ["COMPLETED", "CANCELLED"].contains(&task.data["status"].as_str().unwrap_or_default()) {
                    Vec::new()
                } else {
                    crate::pim::tasks::task_occurrences(resource, zone, from, to + 7 * 86_400_000, false, false)?.into_iter().filter(|o| !["COMPLETED", "CANCELLED"].contains(&o["status"].as_str().unwrap_or_default())).collect()
                }
            } else {
                crate::pim::calendar::expand(resource, zone, from, to + 7 * 86_400_000, false, false)?
            };
            Ok(occurrences
                .into_iter()
                .filter_map(|o| {
                    let minutes = o["reminderMinutes"].as_f64()?;
                    let at = o.get("alarmAt").and_then(Value::as_f64).map_or_else(|| epoch_of(&o["startAt"]), |a| a as i64);
                    let due = at - (minutes * 60_000.0) as i64;
                    (due >= from && due <= to).then(|| json!({ "id": o["recurrenceId"], "dueAt": due, "title": o["title"] }))
                })
                .collect())
        })();
        let result = match planned {
            Ok(reminders) => db.call("service_pim_commit_reminders", json!([resource["id"], resource["revision"], from, to, Value::Array(reminders).to_string()])).await.map(|_| ()),
            Err(e) => Err(e),
        };
        // A concurrent edit is retried from its new revision on the next tick; anything else defers.
        if let Err(error) = result
            && error.code != "conflict"
        {
            let _ = db.call("service_pim_defer_reminder", json!([resource["id"], resource["revision"], from])).await;
            eprintln!("A calendar reminder resource will retry.");
        }
    }
    Ok(())
}

/// One resource per UID with the calendar's time zones; unreadable entries are skipped (`splitSubscription`).
pub fn split_subscription(text: &str) -> Result<(Vec<Value>, usize)> {
    use crate::pim::vobject::{Component, parse, serialize, text as value};
    let root = parse(text.strip_prefix('\u{feff}').unwrap_or(text), "VCALENDAR").map_err(|_| AppError::with("invalidCalendar", 422, "The address did not return an iCalendar (.ics) file."))?;
    let zones: Vec<Component> = root.components("VTIMEZONE").cloned().collect();
    let mut groups: Vec<(String, Vec<Component>)> = Vec::new();
    let mut skipped = 0;
    for c in root.children.iter().filter(|c| c.name == "VEVENT" || c.name == "VTODO") {
        let uid = value(c, "UID").trim().to_owned();
        if uid.is_empty() || uid.encode_utf16().count() > 1024 {
            skipped += 1;
            continue;
        }
        let key = format!("{}/{uid}", c.name);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(c.clone()),
            None => groups.push((key, vec![c.clone()])),
        }
    }
    if groups.len() > 10_000 {
        return Err(AppError::with("operationTooLarge", 413, "The calendar has more than 10,000 events."));
    }
    let mut items = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (key, components) in groups {
        let uid = key.split_once('/').map(|(_, u)| u.to_owned()).unwrap_or_default();
        if seen.contains(&uid) {
            skipped += 1;
            continue;
        }
        let mut calendar = Component { name: "VCALENDAR".into(), ..Default::default() };
        calendar.props.push(crate::pim::vobject::prop("VERSION", &[], "2.0"));
        calendar.props.push(crate::pim::vobject::prop("PRODID", &[], "-//Dar//Calendar subscription//EN"));
        calendar.children.extend(zones.iter().cloned());
        let task = components[0].name == "VTODO";
        calendar.children.extend(components);
        let content = serialize(&calendar);
        if content.len() > 262_144 {
            skipped += 1;
            continue;
        }
        let read = if task { crate::pim::tasks::read_task(&content) } else { crate::pim::calendar::read_event(&content) };
        match read {
            Ok(result) => {
                items.push(json!({ "id": crate::auth::uuid(), "uid": uid, "href": format!("{}.ics", sha1_hex(uid.as_bytes())), "content": result.content, "projection": result.data.to_string() }));
                seen.push(uid);
            }
            Err(_) => skipped += 1,
        }
    }
    Ok((items, skipped))
}

/// Batches stay well under the database call limit.
fn batches(items: &[Value]) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = vec![Vec::new()];
    let mut size = 0;
    for item in items {
        let bytes = item["content"].as_str().map_or(0, str::len) + item["projection"].as_str().map_or(0, str::len) + 200;
        let last = out.last().expect("batch");
        if (size + bytes > 900 * 1024 || last.len() >= 200) && !last.is_empty() {
            out.push(Vec::new());
            size = 0;
        }
        out.last_mut().expect("batch").push(item.clone());
        size += bytes;
    }
    out
}

/// Calendar subscription refreshes: due calendars are fetched (public addresses only unless
/// allowed) and synced by UID. Addresses can carry secrets and are never logged.
#[derive(Clone)]
pub struct Subscriptions {
    db: Database,
    allow_private: bool,
    running: Arc<tokio::sync::Mutex<()>>,
}

impl Subscriptions {
    pub fn new(db: Database, allow_private: bool) -> Subscriptions {
        let subscriptions = Subscriptions { db, allow_private, running: Arc::new(tokio::sync::Mutex::new(())) };
        let ticker = subscriptions.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            loop {
                ticker.tick().await;
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        subscriptions
    }

    /// Runs a refresh soon, for example right after subscribing.
    pub fn poke(&self) {
        let ticker = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            ticker.tick().await;
        });
    }

    async fn worker(&self, operation: &str, value: Value) -> Result<Value> {
        self.db.call("service_pim_subscriptions", json!([operation, value.to_string()])).await
    }

    async fn tick(&self) {
        let Ok(_turn) = self.running.try_lock() else { return };
        let Ok(due) = self.worker("due", json!({})).await else { return };
        for row in due.as_array().into_iter().flatten() {
            self.sync(row).await;
        }
    }

    async fn sync(&self, row: &Value) {
        let collection_id = row["collectionId"].clone();
        let url = row["url"].as_str().unwrap_or_default();
        let (etag, modified) = (row["etag"].as_str().unwrap_or_default(), row["lastModified"].as_str().unwrap_or_default());
        let outcome: Result<()> = async {
            let options = crate::net::FetchOptions { accept: "text/calendar, */*;q=0.1", etag: Some(etag), last_modified: Some(modified), max_bytes: 8 * 1024 * 1024, timeout: Duration::from_secs(30), allow_private: self.allow_private };
            // webcal:// does not name a scheme: HTTPS first, plain HTTP only when HTTPS cannot connect.
            let lower = url.to_lowercase();
            let webcal = lower.starts_with("webcal://");
            let secure = if webcal { format!("https://{}", &url[9..]) } else { url.to_owned() };
            let response = match crate::net::safe_fetch(&secure, &options).await {
                Err(e) if webcal && e.code == "feedUnavailable" => crate::net::safe_fetch(&format!("http://{}", &url[9..]), &options).await?,
                other => other?,
            };
            if response.status == 304 {
                self.worker("finish", json!({ "collectionId": collection_id, "uids": null, "etag": etag, "lastModified": modified })).await?;
                return Ok(());
            }
            if response.status == 401 || response.status == 403 {
                return Err(AppError::with("calendarUnavailable", 502, &format!("The calendar address refused access ({}). Check that the link is still shared.", response.status)));
            }
            if !(200..300).contains(&response.status) {
                return Err(AppError::with("calendarUnavailable", 502, &format!("The calendar address answered {}.", response.status)));
            }
            let (items, skipped) = split_subscription(&String::from_utf8_lossy(&response.body))?;
            for batch in batches(&items) {
                if !batch.is_empty() {
                    self.worker("put", json!({ "collectionId": collection_id, "items": batch })).await?;
                }
            }
            // Only a calendar where nothing could be read is an error; partly readable sources still sync.
            let error = if items.is_empty() && skipped > 0 { format!("Dar could not read the {skipped} {} in this calendar.", if skipped == 1 { "entry" } else { "entries" }) } else { String::new() };
            let uids: Value = if error.is_empty() { json!(items.iter().map(|i| i["uid"].clone()).collect::<Vec<_>>()) } else { Value::Null };
            let (etag, modified) = if error.is_empty() { (response.etag, response.last_modified) } else { (String::new(), String::new()) };
            self.worker("finish", json!({ "collectionId": collection_id, "uids": uids, "etag": etag, "lastModified": modified, "error": error })).await?;
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            let message = if error.message != error.code { error.message } else { "The calendar could not be refreshed.".into() };
            let _ = self.worker("finish", json!({ "collectionId": collection_id, "uids": null, "error": message })).await;
        }
    }
}

/// Content stored under upload IDs gains its digest name, once (`migrateBlobs`).
pub async fn migrate_blobs(db: &Database, root: &Path) -> std::result::Result<(usize, usize), String> {
    let plan = db.call("service_blob_migration_plan", json!([])).await.map_err(|e| e.to_string())?;
    let directory = root.join("blobs");
    let mut migrated = 0;
    let legacy = |id: &str| id.len() == 36 && id.bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-'));
    if plan["done"] != true {
        let mut hashed: HashMap<String, Value> = HashMap::new();
        let mut digests: HashMap<String, String> = HashMap::new();
        for blob in plan["blobs"].as_array().into_iter().flatten() {
            let id = blob["id"].as_str().unwrap_or_default();
            if !legacy(id) {
                continue;
            }
            let source = directory.join(id);
            let meta = tokio::fs::symlink_metadata(&source).await.ok();
            if meta.is_none() && blob["required"] != true {
                continue;
            }
            let Some(meta) = meta.filter(std::fs::Metadata::is_file) else { return Err(format!("Stored content {id} is missing or not a regular file; restore it from a backup, then start Dar again.")) };
            let size = blob["size"].as_f64().unwrap_or(-1.0);
            if size >= 0.0 && meta.len() as f64 != size {
                return Err(format!("Stored content {id} is {} bytes, not the {size} its record says.", meta.len()));
            }
            let mut digest = blob["digest"].as_str().unwrap_or_default().to_owned();
            if digest.is_empty() {
                use sha2::Digest;
                let bytes = tokio::fs::read(&source).await.map_err(|e| e.to_string())?;
                digest = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
                hashed.insert(id.to_owned(), json!({ "digest": digest, "size": meta.len() }));
            }
            let target = directory.join(&digest);
            match tokio::fs::hard_link(&source, &target).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let existing = tokio::fs::symlink_metadata(&target).await.map_err(|e| e.to_string())?;
                    if !existing.is_file() || existing.len() != meta.len() {
                        return Err(format!("Stored content {digest} does not match its other copy."));
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
            digests.insert(id.to_owned(), digest);
            migrated += 1;
        }
        sync_dir(&directory).await;
        for cache in ["photo-cache", "book-cache"] {
            rename_cache(&root.join(cache), &digests).await;
        }
        // A large database may answer after the request's time limit; the reducer is idempotent.
        let hashed = serde_json::to_string(&hashed).expect("hashed");
        for attempt in 1.. {
            match db.call("service_migrate_blobs", json!([hashed])).await {
                Ok(_) => break,
                Err(e) if e.code == "databaseUnavailable" && attempt < 40 => tokio::time::sleep(Duration::from_millis(1500)).await,
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    // After the database switched, upload-ID names are spare links or content nothing used.
    let mut removed = 0;
    if let Ok(mut entries) = tokio::fs::read_dir(&directory).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if legacy(&name) {
                tokio::fs::remove_file(entry.path()).await.map_err(|e| e.to_string())?;
                removed += 1;
            }
        }
    }
    if removed > 0 {
        sync_dir(&directory).await;
        for cache in ["photo-cache", "book-cache"] {
            rename_cache(&root.join(cache), &HashMap::new()).await;
        }
    }
    Ok((migrated, removed))
}

async fn sync_dir(directory: &Path) {
    if let Ok(dir) = tokio::fs::File::open(directory).await {
        let _ = dir.sync_all().await;
    }
}

/// Moves derived files to their digest's name; with no digest they are dropped (caches regenerate).
async fn rename_cache(cache: &Path, digests: &HashMap<String, String>) {
    let Ok(mut prefixes) = tokio::fs::read_dir(cache).await else { return };
    while let Ok(Some(prefix)) = prefixes.next_entry().await {
        let name = prefix.file_name().to_string_lossy().into_owned();
        if name.len() != 2 || !name.bytes().all(|b| b.is_ascii_hexdigit()) || !prefix.path().is_dir() {
            continue;
        }
        let Ok(mut files) = tokio::fs::read_dir(prefix.path()).await else { continue };
        while let Ok(Some(file)) = files.next_entry().await {
            let file_name = file.file_name().to_string_lossy().into_owned();
            let Some((id, suffix)) = file_name.get(..36).zip(file_name.get(37..)).filter(|(id, _)| id.bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-')) && file_name.as_bytes().get(36) == Some(&b'-')) else { continue };
            match digests.get(id) {
                None => {
                    let _ = tokio::fs::remove_file(file.path()).await;
                }
                Some(digest) if is_blob_id(digest) => {
                    let target = cache.join(&digest[..2]);
                    let _ = tokio::fs::DirBuilder::new().recursive(true).mode(0o700).create(&target).await;
                    let _ = tokio::fs::rename(file.path(), target.join(format!("{digest}-{suffix}"))).await;
                }
                Some(_) => {}
            }
        }
    }
}

/// The minute loop: storage, the daily orphan sweep, Git caches, reminders, provider cleanup and the workers.
pub fn start_loop(runtime: crate::http::Shared) {
    tokio::spawn(async move {
        let mut swept: Option<std::time::Instant> = None;
        let mut timer = tokio::time::interval(Duration::from_secs(60));
        timer.tick().await;
        loop {
            timer.tick().await;
            let pass: Result<()> = async {
                runtime.forget_old_attempts();
                reconcile_storage(&runtime.db, &runtime.blobs).await?;
                // Content no upload recorded reads the whole blob directory: once a day, from the first run.
                if swept.is_none_or(|t| t.elapsed() >= Duration::from_secs(86_400)) {
                    swept = Some(std::time::Instant::now());
                    sweep_orphans(&runtime.db, &runtime.blobs).await?;
                }
                runtime.git.maintain().await;
                deliver_reminders(&runtime.db).await?;
                runtime.db.call("service_provider", json!(["cleanup", "{}"])).await?;
                runtime.identity.maintain();
                runtime.checksums.nudge();
                runtime.texts.nudge();
                Ok(())
            }
            .await;
            if pass.is_err() {
                eprintln!("Storage maintenance will retry.");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::split_subscription;

    fn feed() -> String {
        [
            "BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Test//EN",
            "BEGIN:VTIMEZONE", "TZID:Europe/Berlin", "BEGIN:STANDARD", "DTSTART:19701025T030000", "TZOFFSETFROM:+0200", "TZOFFSETTO:+0100", "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU", "END:STANDARD", "END:VTIMEZONE",
            "BEGIN:VEVENT", "UID:offsite@test", "DTSTAMP:20261101T000000Z", "DTSTART;VALUE=DATE:20261117", "DTEND;VALUE=DATE:20261118", "SUMMARY:Company offsite", "END:VEVENT",
            "BEGIN:VEVENT", "UID:standup@test", "DTSTAMP:20261101T000000Z", "DTSTART;TZID=Europe/Berlin:20261116T100000", "DTEND;TZID=Europe/Berlin:20261116T103000", "RRULE:FREQ=DAILY;COUNT=3", "SUMMARY:Standup", "END:VEVENT",
            "BEGIN:VEVENT", "UID:standup@test", "DTSTAMP:20261101T000000Z", "RECURRENCE-ID;TZID=Europe/Berlin:20261117T100000", "DTSTART;TZID=Europe/Berlin:20261117T110000", "DTEND;TZID=Europe/Berlin:20261117T113000", "SUMMARY:Standup (moved)", "END:VEVENT",
            "BEGIN:VEVENT", "DTSTAMP:20261101T000000Z", "DTSTART:20261118T090000Z", "SUMMARY:No UID", "END:VEVENT",
            "BEGIN:VEVENT", "UID:retro@test", "DTSTAMP:20261101T000000Z", "DTSTART:20261118T170000Z", "DTEND:20261118T180000Z", "SUMMARY:Retro", "END:VEVENT",
            "END:VCALENDAR", "",
        ]
        .join("\r\n")
    }

    /// Subscription feeds split by UID, keeping overrides with their series and the zones they use.
    #[test]
    fn subscriptions_split_by_uid() {
        let (items, skipped) = split_subscription(&feed()).expect("a calendar");
        let mut uids: Vec<&str> = items.iter().map(|i| i["uid"].as_str().unwrap_or_default()).collect();
        uids.sort();
        assert_eq!(uids, ["offsite@test", "retro@test", "standup@test"]);
        assert_eq!(skipped, 1);
        let standup = items.iter().find(|i| i["uid"] == "standup@test").expect("standup");
        let content = standup["content"].as_str().unwrap_or_default();
        assert!(content.contains("RECURRENCE-ID") && content.contains("BEGIN:VTIMEZONE"));
        let href = standup["href"].as_str().unwrap_or_default();
        assert!(href.len() == 44 && href.ends_with(".ics") && href[..40].bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(split_subscription("<html>not a calendar</html>").unwrap_err().message.contains("iCalendar"));
    }
}
