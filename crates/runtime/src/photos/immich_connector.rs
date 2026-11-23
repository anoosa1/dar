//! Immich server connector (`next/runtime/photos/immich-connector.ts`): imports originals
//! Dar does not have (by SHA-1) from a user's Immich server into the owner's Photos folder
//! with favorite, archive and description, then mirrors album membership. One run at a time;
//! each is bounded, reports live progress, and the next continues where it stopped.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Datelike;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use crate::auth::uuid;
use crate::blobs::Blobs;
use crate::db::Database;
use crate::digests::Sha1;
use crate::error::AppError;
use crate::photos::runtime::{PhotoJob, PhotosRuntime};
use crate::photos::{
    MAX_FILE_BYTES, MAX_FILE_LABEL, now_ms, parse_iso_ms, photo_kind, sha1_hex_to_base64,
};

const BUDGET_FILES: usize = 300;
const BUDGET_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const BACKLOG_DELAY_MS: u64 = 30_000;
const POLL_INTERVAL_MS: u64 = 5 * 60_000;
const MAX_CONSECUTIVE_FAILURES: usize = 10;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoImmichProgress {
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub imported: usize,
    pub file: String,
    pub started_at: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection {
    user_id: String,
    url: String,
    sealed_key: String,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RemoteExif {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    file_size_in_byte: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteAsset {
    id: String,
    #[serde(default)]
    checksum: String,
    #[serde(default)]
    original_file_name: String,
    #[serde(default)]
    original_mime_type: Option<String>,
    #[serde(default)]
    file_created_at: String,
    #[serde(default)]
    is_favorite: bool,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    is_trashed: bool,
    #[serde(default)]
    exif_info: Option<RemoteExif>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteAlbum {
    id: String,
    #[serde(default)]
    album_name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
struct SyncSummary {
    remote: usize,
    imported: usize,
    albums: usize,
    skipped: usize,
    failed: usize,
}

enum ImportErr {
    Asset(String),
    App(AppError),
    Aborted,
    Other(String),
}

struct Inner {
    db: Database,
    root: PathBuf,
    blobs: Blobs,
    photos: Arc<PhotosRuntime>,
    allow_loopback: bool,
    closing: AtomicBool,
    running: AtomicBool,
    cancel_tx: watch::Sender<bool>,
    progress: Mutex<HashMap<String, PhotoImmichProgress>>,
    backlog: Mutex<HashSet<String>>,
    pending_user: Mutex<Option<String>>,
    wake: Notify,
}

struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub struct ImmichConnector {
    inner: Arc<Inner>,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl ImmichConnector {
    pub fn new(
        db: Database,
        root: &Path,
        blobs: Blobs,
        photos: Arc<PhotosRuntime>,
        allow_loopback: bool,
    ) -> Self {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let inner = Arc::new(Inner {
            db,
            root: root.to_path_buf(),
            blobs,
            photos,
            allow_loopback,
            closing: AtomicBool::new(false),
            running: AtomicBool::new(false),
            cancel_tx,
            progress: Mutex::new(HashMap::new()),
            backlog: Mutex::new(HashSet::new()),
            pending_user: Mutex::new(None),
            wake: Notify::new(),
        });
        let loop_inner = inner.clone();
        let task = tokio::spawn(async move {
            connector_loop(loop_inner, cancel_rx).await;
        });
        Self {
            inner,
            task: tokio::sync::Mutex::new(Some(task)),
        }
    }

    /// Runs one account's connection now (Sync now); ignored if a run is already active.
    pub fn sync_now(&self, user_id: &str) {
        if self.inner.closing.load(Ordering::Relaxed) || self.inner.running.load(Ordering::SeqCst) {
            return;
        }
        let mut pending = self.inner.pending_user.lock().expect("pending");
        if pending.is_none() {
            *pending = Some(user_id.to_owned());
        }
        drop(pending);
        self.inner.wake.notify_one();
    }

    /// Where a running import is, for the Photos status bars; `None` when idle.
    pub fn progress(&self, user_id: &str) -> Option<PhotoImmichProgress> {
        if let Some(value) = self.inner.progress.lock().expect("progress").get(user_id) {
            return Some(value.clone());
        }
        if self.inner.backlog.lock().expect("backlog").contains(user_id) {
            return Some(PhotoImmichProgress {
                phase: "waiting".into(),
                done: 0,
                total: 0,
                imported: 0,
                file: String::new(),
                started_at: 0.0,
            });
        }
        None
    }

    /// Checks an Immich server and key before they are saved.
    pub async fn test(&self, url: &str, key: &str) -> std::result::Result<Value, String> {
        let (_tx, cancel_rx) = watch::channel(false);
        let mut client = ImmichClient::new(
            url,
            key,
            self.inner.allow_loopback,
            Duration::from_secs(15),
            cancel_rx,
        );
        client.json("/server/ping", "GET", None).await.map_err(err_text)?;
        client.json("/users/me", "GET", None).await.map_err(err_text)
    }

    pub async fn close(&self) {
        self.inner.closing.store(true, Ordering::SeqCst);
        let _ = self.inner.cancel_tx.send(true);
        self.inner.wake.notify_one();
        if let Some(handle) = self.task.lock().await.take() {
            let _ = handle.await;
        }
    }
}

fn err_text(e: ImportErr) -> String {
    match e {
        ImportErr::Asset(m) | ImportErr::Other(m) => m,
        ImportErr::App(a) => a.message,
        ImportErr::Aborted => "Stopped.".into(),
    }
}

async fn worker_call(db: &Database, op: &str, value: Value) -> Result<Value, AppError> {
    db.call("service_immich_worker", json!([op, value.to_string()]))
        .await
}

async fn connector_loop(inner: Arc<Inner>, cancel_rx: watch::Receiver<bool>) {
    loop {
        if inner.closing.load(Ordering::Relaxed) {
            break;
        }
        let next_backlog = inner.backlog.lock().expect("backlog").iter().next().cloned();
        let delay = if next_backlog.is_some() {
            Duration::from_millis(BACKLOG_DELAY_MS)
        } else {
            Duration::from_millis(POLL_INTERVAL_MS)
        };
        let requested = inner.pending_user.lock().expect("pending").take();
        let user_id = if let Some(uid) = requested {
            uid
        } else {
            tokio::select! {
                _ = tokio::time::sleep(delay) => next_backlog.unwrap_or_default(),
                _ = inner.wake.notified() => {
                    if inner.closing.load(Ordering::Relaxed) {
                        break;
                    }
                    inner.pending_user.lock().expect("pending").take().unwrap_or_default()
                }
            }
        };
        if inner.closing.load(Ordering::Relaxed) {
            break;
        }
        inner.running.store(true, Ordering::SeqCst);
        let _guard = RunningGuard(&inner.running);
        if let Err(err) = run_tick(&inner, &user_id, cancel_rx.clone()).await
            && !inner.closing.load(Ordering::Relaxed)
        {
            eprintln!("Immich connector: {err}");
        }
    }
}

async fn run_tick(
    inner: &Arc<Inner>,
    user_id: &str,
    cancel_rx: watch::Receiver<bool>,
) -> Result<(), AppError> {
    let due = worker_call(&inner.db, "due", json!({ "userId": user_id })).await?;
    let connections: Vec<Connection> = due
        .get("connections")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    for c in connections {
        if inner.closing.load(Ordering::Relaxed) {
            break;
        }
        run_one(inner, &c, cancel_rx.clone()).await;
    }
    Ok(())
}

fn update_progress(inner: &Inner, user_id: &str, f: impl FnOnce(&mut PhotoImmichProgress)) {
    let mut map = inner.progress.lock().expect("progress");
    if let Some(st) = map.get_mut(user_id) {
        f(st);
    }
}

fn set_phase(inner: &Inner, user_id: &str, name: &str, total: usize) {
    update_progress(inner, user_id, |st| {
        st.phase = name.to_owned();
        st.done = 0;
        st.total = total;
        st.file.clear();
    });
}

async fn run_one(inner: &Arc<Inner>, c: &Connection, cancel_rx: watch::Receiver<bool>) {
    let key = inner.photos.open_immich_key(&c.sealed_key);
    let mut summary = SyncSummary::default();
    let mut first_failure = String::new();
    let mut consecutive = 0usize;

    inner.progress.lock().expect("progress").insert(
        c.user_id.clone(),
        PhotoImmichProgress {
            phase: "connecting".into(),
            done: 0,
            total: 0,
            imported: 0,
            file: String::new(),
            started_at: now_ms(),
        },
    );

    let run_result: Result<(), ImportErr> = async {
        if key.is_empty() {
            return Err(ImportErr::Other(
                "The API key cannot be read on this installation. Enter it again.".into(),
            ));
        }
        let mut client = ImmichClient::new(
            &c.url,
            &key,
            inner.allow_loopback,
            Duration::from_secs(10 * 60),
            cancel_rx,
        );
        client.json("/users/me", "GET", None).await?;

        // What Dar already has: imported Immich IDs, and the SHA-1 of the owner's photos.
        set_phase(inner, &c.user_id, "comparing", 0);
        let mut mapped: HashMap<String, String> = HashMap::new();
        let mut local: HashMap<String, String> = HashMap::new(); // sha1 -> fileId
        let mut after = String::new();
        for _ in 0..1000 {
            let known = worker_call(
                &inner.db,
                "known",
                json!({ "userId": c.user_id, "after": after, "limit": 2000 }),
            )
            .await
            .map_err(ImportErr::App)?;
            if let Some(remotes) = known.get("remote").and_then(Value::as_array) {
                for r in remotes {
                    if let (Some(rid), Some(fid)) = (
                        r.get("remoteId").and_then(Value::as_str),
                        r.get("fileId").and_then(Value::as_str),
                    ) {
                        mapped.insert(rid.to_owned(), fid.to_owned());
                    }
                }
            }
            if let Some(locals) = known.get("local").and_then(Value::as_array) {
                for l in locals {
                    if let (Some(sha1), Some(fid)) = (
                        l.get("sha1").and_then(Value::as_str),
                        l.get("fileId").and_then(Value::as_str),
                    ) {
                        local.insert(sha1.to_owned(), fid.to_owned());
                    }
                }
            }
            let next = known.get("next").and_then(Value::as_str).unwrap_or("");
            if next.is_empty() {
                break;
            }
            after = next.to_owned();
        }

        // Import: page through Immich's library.
        set_phase(inner, &c.user_id, "importing", 0);
        let mut files = 0usize;
        let mut bytes = 0u64;
        for page in 1..10_000usize {
            if inner.closing.load(Ordering::Relaxed) {
                break;
            }
            let result = client
                .json(
                    "/search/metadata",
                    "POST",
                    Some(json!({ "page": page, "size": 500, "withExif": true, "withDeleted": false })),
                )
                .await?;
            let assets_obj = result.get("assets").cloned().unwrap_or_else(|| json!({}));
            if let Some(total) = assets_obj.get("total").and_then(Value::as_f64)
                && total > 0.0
            {
                let t = total as usize;
                update_progress(inner, &c.user_id, |st| st.total = t);
            }
            let items: Vec<RemoteAsset> = assets_obj
                .get("items")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            for asset in items {
                summary.remote += 1;
                update_progress(inner, &c.user_id, |st| {
                    st.done += 1;
                    if st.total < st.done {
                        st.total = st.done;
                    }
                });
                if asset.is_trashed
                    || asset.visibility == "locked"
                    || mapped.contains_key(&asset.id)
                {
                    continue;
                }
                if let Some(existing_file_id) = local.get(&asset.checksum).cloned() {
                    mapped.insert(asset.id.clone(), existing_file_id.clone());
                    worker_call(
                        &inner.db,
                        "map",
                        json!({
                            "userId": c.user_id,
                            "remoteId": asset.id,
                            "fileId": existing_file_id,
                            "checksum": asset.checksum,
                        }),
                    )
                    .await
                    .map_err(ImportErr::App)?;
                    continue;
                }
                if files >= BUDGET_FILES || bytes >= BUDGET_BYTES {
                    summary.skipped += 1;
                    continue;
                }
                update_progress(inner, &c.user_id, |st| {
                    st.file = asset.original_file_name.clone();
                });
                match import_asset(inner, c, &mut client, &asset).await {
                    Ok((file_id, size)) => {
                        files += 1;
                        bytes += size;
                        summary.imported += 1;
                        update_progress(inner, &c.user_id, |st| st.imported += 1);
                        consecutive = 0;
                        mapped.insert(asset.id.clone(), file_id.clone());
                        worker_call(
                            &inner.db,
                            "map",
                            json!({
                                "userId": c.user_id,
                                "remoteId": asset.id,
                                "fileId": file_id,
                                "checksum": asset.checksum,
                            }),
                        )
                        .await
                        .map_err(ImportErr::App)?;
                    }
                    Err(err) => {
                        if inner.closing.load(Ordering::Relaxed) {
                            return Err(ImportErr::Aborted);
                        }
                        match &err {
                            ImportErr::Aborted => return Err(err),
                            ImportErr::App(a) if a.code == "quotaExceeded" => return Err(err),
                            _ => {}
                        }
                        summary.failed += 1;
                        let reason = match &err {
                            ImportErr::Asset(msg) => msg.clone(),
                            ImportErr::App(a) => match a.code.as_str() {
                                "invalidInput" => "Dar cannot store it".into(),
                                "invalidName" => "its name is not allowed in Dar".into(),
                                "conflict" => "a file with its name could not be created".into(),
                                _ => a.message.clone(),
                            },
                            ImportErr::Other(msg) => msg.clone(),
                            ImportErr::Aborted => "Stopped.".into(),
                        };
                        if first_failure.is_empty() {
                            let label = if asset.original_file_name.is_empty() {
                                asset.id.as_str()
                            } else {
                                asset.original_file_name.as_str()
                            };
                            first_failure = format!("{label}: {reason}");
                        }
                        consecutive += 1;
                        if consecutive >= MAX_CONSECUTIVE_FAILURES {
                            return Err(ImportErr::Other(format!(
                                "{consecutive} files in a row could not be imported. {first_failure}"
                            )));
                        }
                    }
                }
            }
            let next_page = assets_obj
                .get("nextPage")
                .and_then(|v| if v.is_null() { None } else { Some(v) });
            if next_page.is_none() {
                break;
            }
        }

        // Albums: membership of imported photos, by Immich album.
        let albums_val = client.json("/albums", "GET", None).await?;
        let albums: Vec<RemoteAlbum> =
            serde_json::from_value(albums_val).unwrap_or_default();
        set_phase(inner, &c.user_id, "albums", albums.len());
        for album in albums {
            if inner.closing.load(Ordering::Relaxed) {
                break;
            }
            update_progress(inner, &c.user_id, |st| {
                st.done += 1;
                st.file = album.album_name.clone();
            });
            let mut file_ids: Vec<String> = Vec::new();
            for page in 1..1000usize {
                let result = client
                    .json(
                        "/search/metadata",
                        "POST",
                        Some(json!({ "page": page, "size": 1000, "albumIds": [album.id] })),
                    )
                    .await?;
                let assets_obj = result.get("assets").cloned().unwrap_or_else(|| json!({}));
                let items: Vec<RemoteAsset> = assets_obj
                    .get("items")
                    .cloned()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                for a in items {
                    if let Some(fid) = mapped.get(&a.id).or_else(|| local.get(&a.checksum)) {
                        file_ids.push(fid.clone());
                    }
                }
                let next_page = assets_obj
                    .get("nextPage")
                    .and_then(|v| if v.is_null() { None } else { Some(v) });
                if next_page.is_none() {
                    break;
                }
            }
            let res = worker_call(
                &inner.db,
                "album",
                json!({
                    "userId": c.user_id,
                    "remoteId": album.id,
                    "name": album.album_name,
                    "description": album.description.unwrap_or_default(),
                    "fileIds": file_ids,
                }),
            )
            .await
            .map_err(ImportErr::App)?;
            if res.get("albumId").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
                summary.albums += 1;
            }
        }
        Ok(())
    }
    .await;

    let error = match run_result {
        Ok(()) => String::new(),
        Err(ImportErr::Aborted) => "Stopped.".into(),
        Err(ImportErr::App(ref a)) if a.code == "quotaExceeded" => {
            "Your storage limit is reached; the import continues once there is space.".into()
        }
        Err(other) => err_text(other).chars().take(300).collect(),
    };
    inner.progress.lock().expect("progress").remove(&c.user_id);
    if error.is_empty() && summary.skipped > 0 {
        inner.backlog.lock().expect("backlog").insert(c.user_id.clone());
    } else {
        inner.backlog.lock().expect("backlog").remove(&c.user_id);
    }
    let failed_msg = if summary.failed > 0 {
        let noun = if summary.failed == 1 { "file" } else { "files" };
        let short_first: String = first_failure.chars().take(200).collect();
        let more = if summary.failed > 1 { " and others" } else { "" };
        format!("{} {noun} could not be imported ({short_first}{more}).", summary.failed)
    } else {
        String::new()
    };
    let combined = match (error.is_empty(), failed_msg.is_empty()) {
        (true, true) => String::new(),
        (false, true) => error,
        (true, false) => failed_msg,
        (false, false) => format!("{error} {failed_msg}"),
    };
    let _ = worker_call(
        &inner.db,
        "report",
        json!({ "userId": c.user_id, "error": combined, "summary": summary }),
    )
    .await;
}

async fn import_asset(
    inner: &Arc<Inner>,
    c: &Connection,
    client: &mut ImmichClient,
    asset: &RemoteAsset,
) -> Result<(String, u64), ImportErr> {
    let raw_name = if asset.original_file_name.is_empty() {
        "immich"
    } else {
        asset.original_file_name.as_str()
    };
    let cleaned: String = raw_name
        .chars()
        .map(|ch| {
            if (ch as u32) <= 0x1f || ch == '\\' || ch == '/' {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(255)
        .collect();
    let name = if cleaned.is_empty() { "immich".to_owned() } else { cleaned };
    let media_type = asset
        .original_mime_type
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("application/octet-stream");
    if photo_kind(media_type, &name).is_none() {
        return Err(ImportErr::Asset(format!(
            "{media_type} is not a photo or video type"
        )));
    }
    if asset
        .exif_info
        .as_ref()
        .and_then(|e| e.file_size_in_byte)
        .unwrap_or(0.0)
        > MAX_FILE_BYTES as f64
    {
        return Err(ImportErr::Asset(format!(
            "it is larger than {MAX_FILE_LABEL}, Dar's file size limit"
        )));
    }
    let dir = inner
        .root
        .join("uploads")
        .join(format!("immich-import-{}", uuid()));
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .await
        .map_err(|e| ImportErr::Other(e.to_string()))?;

    let result = async {
        let file_path = dir.join("original");
        client
            .download_to(&format!("/assets/{}/original", asset.id), &file_path, MAX_FILE_BYTES)
            .await?;
        let mut f = tokio::fs::File::open(&file_path)
            .await
            .map_err(|e| ImportErr::Other(e.to_string()))?;
        let mut sha1 = Sha1::default();
        let mut buf = vec![0u8; 1 << 20];
        let mut size = 0u64;
        loop {
            let n = f
                .read(&mut buf)
                .await
                .map_err(|e| ImportErr::Other(e.to_string()))?;
            if n == 0 {
                break;
            }
            size += n as u64;
            if size > MAX_FILE_BYTES {
                return Err(ImportErr::Asset(format!(
                    "it is larger than {MAX_FILE_LABEL}, Dar's file size limit"
                )));
            }
            sha1.update(&buf[..n]);
        }
        drop(f);
        let sha1_b64 = sha1_hex_to_base64(&sha1.hex());

        let ym_re = Regex::new(r"^(\d{4})-(\d{2})").expect("regex");
        let now = chrono::Utc::now();
        let (year, month) = ym_re
            .captures(&asset.file_created_at)
            .and_then(|caps| {
                let y: i32 = caps[1].parse().ok()?;
                let m: u32 = caps[2].parse().ok()?;
                ((1900..=2999).contains(&y) && (1..=12).contains(&m)).then_some((y, m))
            })
            .unwrap_or_else(|| (now.year(), now.month()));

        let begun = worker_call(
            &inner.db,
            "importBegin",
            json!({
                "userId": c.user_id,
                "sha1": sha1_b64,
                "name": name,
                "size": size,
                "mediaType": media_type,
                "year": year,
                "month": month,
            }),
        )
        .await
        .map_err(ImportErr::App)?;
        if let Some(dup) = begun.get("duplicate").and_then(Value::as_str).filter(|s| !s.is_empty())
        {
            return Ok((dup.to_owned(), 0));
        }
        let upload_id = begun.get("id").and_then(Value::as_str).unwrap_or("");
        let f = tokio::fs::File::open(&file_path)
            .await
            .map_err(|e| ImportErr::Other(e.to_string()))?;
        let digest = inner
            .blobs
            .receive(tokio_util::io::ReaderStream::new(f), size)
            .await
            .map_err(ImportErr::App)?;
        let mut finish_arg = json!({
            "userId": c.user_id,
            "id": upload_id,
            "digest": digest,
        });
        if let Some(ft) = parse_iso_ms(&asset.file_created_at) {
            finish_arg
                .as_object_mut()
                .expect("obj")
                .insert("fileTime".into(), json!(ft));
        }
        let finished = worker_call(&inner.db, "importFinish", finish_arg)
            .await
            .map_err(ImportErr::App)?;
        let file_id = finished
            .get("fileId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let job: Option<PhotoJob> = finished
            .get("job")
            .filter(|v| !v.is_null())
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());
        let _ = inner.photos.index_now(job).await;
        let desc = asset
            .exif_info
            .as_ref()
            .and_then(|e| e.description.as_deref())
            .unwrap_or("");
        worker_call(
            &inner.db,
            "apply",
            json!({
                "userId": c.user_id,
                "fileId": file_id,
                "favorite": asset.is_favorite,
                "archived": asset.visibility == "archive",
                "description": desc,
            }),
        )
        .await
        .map_err(ImportErr::App)?;
        Ok((file_id, size))
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&dir).await;
    result
}

/// Minimal Immich API client backed by `curl` with IP pinning (`--resolve`), TLS support,
/// same-host redirect following, and SSRF validation at every hop.
struct ImmichClient {
    base: String,
    key: String,
    allow_loopback: bool,
    timeout: Duration,
    cancel_rx: watch::Receiver<bool>,
}

impl ImmichClient {
    fn new(
        url: &str,
        key: &str,
        allow_loopback: bool,
        timeout: Duration,
        cancel_rx: watch::Receiver<bool>,
    ) -> Self {
        let trimmed = url.trim_end_matches('/');
        let base_root = trimmed.strip_suffix("/api").unwrap_or(trimmed);
        Self {
            base: format!("{base_root}/api"),
            key: key.to_owned(),
            allow_loopback,
            timeout,
            cancel_rx,
        }
    }

    async fn json(
        &mut self,
        route: &str,
        method: &str,
        body: Option<Value>,
    ) -> Result<Value, ImportErr> {
        let work = make_temp_dir().await?;
        let out_file = work.join("body");
        let res = self
            .transfer(route, method, body, &out_file, 64 * 1024 * 1024)
            .await;
        let parsed = match res {
            Ok(()) => {
                let bytes = tokio::fs::read(&out_file)
                    .await
                    .map_err(|e| ImportErr::Other(e.to_string()));
                bytes.and_then(|b| {
                    serde_json::from_slice::<Value>(&b)
                        .map_err(|e| ImportErr::Other(format!("Invalid JSON from Immich: {e}")))
                })
            }
            Err(e) => Err(e),
        };
        let _ = tokio::fs::remove_dir_all(&work).await;
        parsed
    }

    async fn download_to(
        &mut self,
        route: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), ImportErr> {
        self.transfer(route, "GET", None, dest, max_bytes).await
    }

    async fn transfer(
        &mut self,
        route: &str,
        method: &str,
        body: Option<Value>,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), ImportErr> {
        let mut target = format!("{}{route}", self.base);
        for _ in 0..5 {
            if *self.cancel_rx.borrow() {
                return Err(ImportErr::Aborted);
            }
            let (host, port, ip) = allowed_host(&target, self.allow_loopback)
                .await
                .map_err(ImportErr::Other)?;
            let work = make_temp_dir().await?;
            let headers_file = work.join("headers");
            let config_file = work.join("curl.cfg");
            let req_body_file = work.join("req.json");

            let hop_result = async {
                // Keep x-api-key out of /proc/<pid>/cmdline by passing it via a 0600 curl config file.
                let safe_key = self
                    .key
                    .replace(['\r', '\n'], "")
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                let cfg_text = format!("header = \"x-api-key: {safe_key}\"\n");
                let mut cf = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&config_file)
                    .await
                    .map_err(|e| ImportErr::Other(e.to_string()))?;
                cf.write_all(cfg_text.as_bytes())
                    .await
                    .map_err(|e| ImportErr::Other(e.to_string()))?;
                drop(cf);

                let pin = match ip {
                    IpAddr::V4(v4) => format!("{host}:{port}:{v4}"),
                    IpAddr::V6(v6) => format!("{host}:{port}:[{v6}]"),
                };
                let mut cmd = Command::new("curl");
                cmd.args([
                    "--silent",
                    "--show-error",
                    "--proto",
                    "=http,https",
                    "--max-redirs",
                    "0",
                    "--globoff",
                ])
                .arg("--max-time")
                .arg(self.timeout.as_secs().max(1).to_string())
                .arg("--max-filesize")
                .arg(max_bytes.to_string())
                .arg("--resolve")
                .arg(&pin)
                .arg("-K")
                .arg(&config_file)
                .args(["-X", method, "-H", "accept: application/json"])
                .arg("-D")
                .arg(&headers_file)
                .arg("-o")
                .arg(dest);
                if let Some(ref payload) = body {
                    let mut rf = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&req_body_file)
                        .await
                        .map_err(|e| ImportErr::Other(e.to_string()))?;
                    rf.write_all(payload.to_string().as_bytes())
                        .await
                        .map_err(|e| ImportErr::Other(e.to_string()))?;
                    drop(rf);
                    cmd.args(["-H", "content-type: application/json"])
                        .arg("--data-binary")
                        .arg(format!("@{}", req_body_file.display()));
                }
                cmd.arg("--")
                    .arg(&target)
                    .env_clear()
                    .env("PATH", std::env::var("PATH").unwrap_or_default())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);

                let mut child = cmd
                    .spawn()
                    .map_err(|_| ImportErr::Other("Immich could not be reached (curl is missing).".into()))?;
                let mut stderr_pipe = child.stderr.take();
                let mut cancel = self.cancel_rx.clone();
                let status = tokio::select! {
                    st = child.wait() => st.map_err(|e| ImportErr::Other(e.to_string()))?,
                    changed = cancel.changed() => {
                        if changed.is_err() || *cancel.borrow() {
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            return Err(ImportErr::Aborted);
                        }
                        child.wait().await.map_err(|e| ImportErr::Other(e.to_string()))?
                    }
                };
                let mut stderr_buf = Vec::new();
                if let Some(mut p) = stderr_pipe.take() {
                    let _ = p.read_to_end(&mut stderr_buf).await;
                }
                let stderr_text = String::from_utf8_lossy(&stderr_buf).into_owned();
                match status.code() {
                    Some(0) => {}
                    Some(63) => {
                        return Err(ImportErr::Asset(format!(
                            "it is larger than {MAX_FILE_LABEL}, Dar's file size limit"
                        )));
                    }
                    code => {
                        return Err(ImportErr::Other(curl_network_message(
                            code,
                            &stderr_text,
                            &target,
                        )));
                    }
                }
                let raw_headers = tokio::fs::read_to_string(&headers_file)
                    .await
                    .unwrap_or_default();
                let block = raw_headers
                    .trim_end()
                    .rsplit("\r\n\r\n")
                    .next()
                    .unwrap_or_default();
                let mut lines = block.lines();
                let http_status: u16 = lines
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let mut location = String::new();
                for line in lines {
                    if let Some((k, v)) = line.split_once(':')
                        && k.trim().eq_ignore_ascii_case("location")
                    {
                        location = v.trim().to_owned();
                    }
                }
                Ok((http_status, location))
            }
            .await;

            let _ = tokio::fs::remove_dir_all(&work).await;
            let (http_status, location) = hop_result?;
            if [301, 302, 303, 307, 308].contains(&http_status) {
                let target_url = url::Url::parse(&target).ok();
                let next = target_url
                    .as_ref()
                    .and_then(|u| (!location.is_empty()).then(|| u.join(&location).ok()).flatten());
                let Some(next) = next else {
                    return Err(ImportErr::Other(
                        "The server redirects to nowhere. Enter that address instead.".into(),
                    ));
                };
                if next.host_str() != target_url.as_ref().and_then(|u| u.host_str()) {
                    let host_port = host_port_of(&next);
                    return Err(ImportErr::Other(format!(
                        "The server redirects to {host_port}. Enter that address instead."
                    )));
                }
                if next.path().ends_with(route) {
                    let origin_str = next.origin().ascii_serialization();
                    let prefix = &next.path()[..next.path().len() - route.len()];
                    self.base = format!("{origin_str}{prefix}");
                }
                target = next.to_string();
                continue;
            }
            if !(200..300).contains(&http_status) {
                if http_status == 401 || http_status == 403 {
                    return Err(ImportErr::Other("Immich rejected the API key.".into()));
                }
                return Err(ImportErr::Other(format!(
                    "Immich answered {http_status} for {route}."
                )));
            }
            return Ok(());
        }
        Err(ImportErr::Other("The server redirected too many times.".into()))
    }
}

fn host_port_of(u: &url::Url) -> String {
    match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_owned(),
        _ => u.as_str().to_owned(),
    }
}

fn curl_network_message(code: Option<i32>, stderr: &str, target: &str) -> String {
    let host = url::Url::parse(target)
        .map(|u| host_port_of(&u))
        .unwrap_or_else(|_| target.to_owned());
    let lower = stderr.to_ascii_lowercase();
    if code == Some(28) {
        return format!("{host} did not answer in time.");
    }
    eprintln!("Immich connector: {host}: curl({code:?}) {}", stderr.trim());
    match code {
        Some(7) => {
            if lower.contains("unreachable") || lower.contains("no route") {
                format!(
                    "The Dar server cannot reach {host} (EHOSTUNREACH). Check that it is reachable from the Dar server, not just from your browser."
                )
            } else {
                format!(
                    "Nothing accepted the connection at {host}. Check the address and port (Immich listens on 2283 unless a proxy is in front)."
                )
            }
        }
        Some(6) => format!("The Dar server cannot resolve {host}."),
        Some(60 | 51 | 58 | 77) => format!(
            "The HTTPS certificate of {host} is not trusted by the Dar server (curl error {}). Use a certificate from a trusted authority, or the server's http:// address on your network.",
            code.unwrap_or(60)
        ),
        Some(35) => format!("{host} does not speak HTTPS on that port. Use http:// for it."),
        Some(52 | 56) => format!("{host} closed the connection. If it serves HTTPS, use https://."),
        Some(c) => format!("Immich could not be reached at {host} (curl error {c})."),
        None => format!("Immich could not be reached at {host} (network error)."),
    }
}

async fn make_temp_dir() -> Result<PathBuf, ImportErr> {
    let dir = std::env::temp_dir().join(format!("dar-immich-{}", uuid()));
    tokio::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .await
        .map_err(|e| ImportErr::Other(e.to_string()))?;
    Ok(dir)
}

fn in_v4(a: [u8; 4], net: [u8; 4], bits: u32) -> bool {
    let (a, net) = (u32::from_be_bytes(a), u32::from_be_bytes(net));
    let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    a & mask == net & mask
}

fn in_v6(a: [u16; 8], net: [u16; 8], bits: u32) -> bool {
    let to = |s: [u16; 8]| s.iter().fold(0u128, |acc, p| (acc << 16) | *p as u128);
    let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
    to(a) & mask == to(net) & mask
}

/// Immich servers usually live on the local network, so RFC 1918 / ULA private addresses are
/// allowed. Loopback, link-local (cloud metadata), unspecified and multicast addresses are not,
/// unless `allow_loopback` is enabled (development / explicit operator setting).
fn is_blocked_address(addr: IpAddr, allow_loopback: bool) -> bool {
    const BLOCKED_V4: [([u8; 4], u32); 5] = [
        ([0, 0, 0, 0], 8),
        ([127, 0, 0, 0], 8),
        ([169, 254, 0, 0], 16),
        ([224, 0, 0, 0], 4),
        ([240, 0, 0, 0], 4),
    ];
    const BLOCKED_V6: [([u16; 8], u32); 4] = [
        ([0; 8], 128),
        ([0, 0, 0, 0, 0, 0, 0, 1], 128),
        ([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),
        ([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),
    ];
    match addr {
        IpAddr::V4(v4) => {
            if allow_loopback && v4.is_loopback() {
                return false;
            }
            BLOCKED_V4.iter().any(|(net, bits)| in_v4(v4.octets(), *net, *bits))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_address(IpAddr::V4(v4), allow_loopback);
            }
            if allow_loopback && v6.is_loopback() {
                return false;
            }
            BLOCKED_V6.iter().any(|(net, bits)| in_v6(v6.segments(), *net, *bits))
        }
    }
}

async fn allowed_host(base: &str, allow_loopback: bool) -> Result<(String, u16, IpAddr), String> {
    let url = url::Url::parse(base)
        .map_err(|_| "Use an http or https address without credentials.".to_owned())?;
    if !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use an http or https address without credentials.".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "Use an http or https address without credentials.".to_owned())?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = url.port_or_known_default().unwrap_or(80);
    let addresses: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| "The Immich server's name could not be resolved.".to_owned())?
            .map(|a| a.ip())
            .collect(),
    };
    if addresses.is_empty() {
        return Err("The Immich server's name could not be resolved.".into());
    }
    for &addr in &addresses {
        if is_blocked_address(addr, allow_loopback) {
            return Err("That address points to this server or a reserved network. For Immich on the same machine, the administrator can set services.dar.photos.immichAllowLocalHost and you can use http://localhost:2283 (or http://[::1]:2283 if Immich listens on IPv6 only).".into());
        }
    }
    Ok((host, port, addresses[0]))
}
