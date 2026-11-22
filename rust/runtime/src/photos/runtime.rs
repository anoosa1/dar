//! Photos background runtime (`next/runtime/photos/runtime.ts`): bounded Files scan for new
//! or replaced media, fact extraction, thumbnails, perceptual hashes, browser-playable videos,
//! KeyBox sealing for shared links and Immich API keys, and serialized link-password hashing.

use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::sync::{Notify, Semaphore, watch};
use tokio::task::JoinHandle;

use crate::auth::{self, KeyBox, secrets_dir};
use crate::blobs::is_blob_id;
use crate::db::Database;
use crate::digests::Sha1;
use crate::error::{AppError, Result};
use crate::photos::media::{cache_path, make_thumbnail, make_video, perceptual_hash, read_facts, video_path};
use crate::photos::{now_ms, sha1_hex_to_base64};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoJob {
    pub file_id: String,
    pub blob_id: String,
    pub name: String,
    pub media_type: String,
    pub modified_at: f64,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct VideoRequest {
    pub blob_id: String,
    pub mode: String,
    pub audio: String,
    pub duration: f64,
}

#[derive(Clone, Debug)]
pub struct VideoStatus {
    pub state: &'static str,
    pub progress: f64,
    pub file: Option<PathBuf>,
}

struct VideoEntry {
    state: &'static str,
    progress: f64,
    until: Option<f64>,
}

struct Inner {
    db: Database,
    root: PathBuf,
    cache: PathBuf,
    links: KeyBox,
    immich_keys: KeyBox,
    /// scrypt uses ~128 MiB per run: link passwords are hashed and checked one at a time.
    hashing: Semaphore,
    /// Media tools are CPU heavy: two at a time across scans and on-demand requests.
    slots: Semaphore,
    closing: AtomicBool,
    cancel_tx: watch::Sender<bool>,
    scan_notify: Notify,
    scan_state: Mutex<(bool, f64)>, // (running, last_pass_ms)
    inflight: Mutex<HashMap<String, watch::Receiver<Option<bool>>>>,
    inflight_done: Notify,
    videos: Mutex<HashMap<String, VideoEntry>>,
    video_queue: Mutex<VecDeque<VideoRequest>>,
    video_notify: Notify,
}

pub struct PhotosRuntime {
    inner: Arc<Inner>,
    scan_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    video_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl PhotosRuntime {
    pub async fn open(db: Database, root: &Path) -> std::io::Result<Self> {
        let cache = root.join("photo-cache");
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&cache)
            .await?;
        let secrets = secrets_dir(root);
        let links = KeyBox::open(&secrets, "photo-links")?;
        let immich_keys = KeyBox::open(&secrets, "immich-api-keys")?;
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let inner = Arc::new(Inner {
            db,
            root: root.to_path_buf(),
            cache,
            links,
            immich_keys,
            hashing: Semaphore::new(1),
            slots: Semaphore::new(2),
            closing: AtomicBool::new(false),
            cancel_tx,
            scan_notify: Notify::new(),
            scan_state: Mutex::new((false, 0.0)),
            inflight: Mutex::new(HashMap::new()),
            inflight_done: Notify::new(),
            videos: Mutex::new(HashMap::new()),
            video_queue: Mutex::new(VecDeque::new()),
            video_notify: Notify::new(),
        });

        let scan_inner = inner.clone();
        let scan_task = tokio::spawn(async move {
            scan_loop(scan_inner).await;
        });

        let video_inner = inner.clone();
        let video_task = tokio::spawn(async move {
            video_loop(video_inner, cancel_rx).await;
        });

        Ok(Self {
            inner,
            scan_task: tokio::sync::Mutex::new(Some(scan_task)),
            video_task: tokio::sync::Mutex::new(Some(video_task)),
        })
    }

    /// Opening Photos rescans soon, at most every 5 seconds, so files added in Files appear quickly.
    pub fn kick(&self) {
        if self.inner.closing.load(Ordering::Relaxed) {
            return;
        }
        let state = self.inner.scan_state.lock().expect("scan_state");
        if !state.0 && now_ms() - state.1 > 5_000.0 {
            drop(state);
            self.inner.scan_notify.notify_one();
        }
    }

    /// Indexes one file now (after a Photos upload); returns whether it was indexed.
    pub async fn index_now(&self, job: Option<PhotoJob>) -> Result<bool> {
        let Some(job) = job else {
            return Ok(false);
        };
        index_job(&self.inner, &job).await?;
        Ok(true)
    }

    /// Path of a ready thumbnail, generating it on demand.
    pub async fn thumbnail_file(&self, blob_id: &str, kind: &str, size: &str) -> Option<PathBuf> {
        let size = if size == "m" { "m" } else { "s" };
        let kind = if kind == "video" { "video" } else { "image" };
        if run_thumbnail(&self.inner, blob_id, kind, size).await {
            cache_path(&self.inner.cache, blob_id, size).ok()
        } else {
            None
        }
    }

    /// Starts (or reports) preparing a browser-playable copy.
    pub async fn video(&self, request: VideoRequest) -> VideoStatus {
        let Ok(file) = video_path(&self.inner.cache, &request.blob_id) else {
            return VideoStatus { state: "failed", progress: 0.0, file: None };
        };
        if tokio::fs::metadata(&file).await.is_ok_and(|m| m.len() > 0) {
            return VideoStatus { state: "ready", progress: 1.0, file: Some(file) };
        }
        let now = now_ms();
        {
            let videos = self.inner.videos.lock().expect("videos");
            if let Some(known) = videos.get(&request.blob_id)
                && (known.state != "failed" || known.until.unwrap_or(0.0) > now)
            {
                return VideoStatus { state: known.state, progress: known.progress, file: None };
            }
        }
        let mut queue = self.inner.video_queue.lock().expect("video_queue");
        if queue.len() >= 50 {
            return VideoStatus { state: "failed", progress: 0.0, file: None };
        }
        {
            let mut videos = self.inner.videos.lock().expect("videos");
            // Re-check under lock to prevent duplicate enqueueing.
            if let Some(known) = videos.get(&request.blob_id)
                && (known.state != "failed" || known.until.unwrap_or(0.0) > now)
            {
                return VideoStatus { state: known.state, progress: known.progress, file: None };
            }
            videos.insert(
                request.blob_id.clone(),
                VideoEntry { state: "preparing", progress: 0.0, until: None },
            );
        }
        queue.push_back(request);
        drop(queue);
        self.inner.video_notify.notify_one();
        VideoStatus { state: "preparing", progress: 0.0, file: None }
    }

    pub fn video_file(&self, blob_id: &str) -> std::io::Result<PathBuf> {
        video_path(&self.inner.cache, blob_id)
    }

    pub fn seal_link(&self, token: &str) -> String {
        self.inner.links.encrypt(&json!({ "token": token }))
    }

    /// The token of a sealed link, or `""` if this instance cannot decrypt it.
    pub fn open_link(&self, sealed: &str) -> String {
        self.inner
            .links
            .decrypt(sealed)
            .and_then(|v| v.get("token").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default()
    }

    /// Immich server API keys live in the database only in this sealed form.
    pub fn seal_immich_key(&self, key: &str) -> String {
        self.inner.immich_keys.encrypt(&json!({ "key": key }))
    }

    pub fn open_immich_key(&self, sealed: &str) -> String {
        self.inner
            .immich_keys
            .decrypt(sealed)
            .and_then(|v| v.get("key").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default()
    }

    pub async fn hash_link_password(&self, password: String) -> Result<String> {
        let _permit = self.inner.hashing.acquire().await.map_err(|_| AppError::internal())?;
        tokio::task::spawn_blocking(move || auth::hash_password(&password))
            .await
            .map_err(|_| AppError::internal())
    }

    pub async fn check_link_password(&self, password: String, verifier: String) -> Result<bool> {
        let _permit = self.inner.hashing.acquire().await.map_err(|_| AppError::internal())?;
        tokio::task::spawn_blocking(move || auth::verify_password(&password, &verifier))
            .await
            .map_err(|_| AppError::internal())
    }

    pub async fn close(&self) {
        self.inner.closing.store(true, Ordering::SeqCst);
        let _ = self.inner.cancel_tx.send(true);
        self.inner.scan_notify.notify_one();
        self.inner.video_notify.notify_one();
        if let Some(handle) = self.scan_task.lock().await.take() {
            let _ = handle.await;
        }
        if let Some(handle) = self.video_task.lock().await.take() {
            let _ = handle.await;
        }
        loop {
            let notified = self.inner.inflight_done.notified();
            if self.inner.inflight.lock().expect("inflight").is_empty() {
                break;
            }
            notified.await;
        }
    }
}

fn blob_path(root: &Path, blob_id: &str) -> Result<PathBuf> {
    if !is_blob_id(blob_id) {
        return Err(AppError::new("invalidBlobId", 400));
    }
    Ok(root.join("blobs").join(blob_id))
}

async fn run_thumbnail(inner: &Arc<Inner>, blob_id: &str, kind: &str, size: &str) -> bool {
    let Ok(source) = blob_path(&inner.root, blob_id) else {
        return false;
    };
    let key = format!("{blob_id}-{size}");
    let mut rx = {
        let mut inflight = inner.inflight.lock().expect("inflight");
        if let Some(existing) = inflight.get(&key) {
            existing.clone()
        } else {
            let (tx, rx) = watch::channel(None);
            inflight.insert(key.clone(), rx.clone());
            let inner = inner.clone();
            let blob_id = blob_id.to_owned();
            let kind = kind.to_owned();
            let size = size.to_owned();
            tokio::spawn(async move {
                let ok = match inner.slots.acquire().await {
                    Ok(_permit) => make_thumbnail(&inner.cache, &source, &blob_id, &kind, &size).await,
                    Err(_) => false,
                };
                let _ = tx.send(Some(ok));
                let mut map = inner.inflight.lock().expect("inflight");
                map.remove(&key);
                if map.is_empty() {
                    inner.inflight_done.notify_waiters();
                }
            });
            rx
        }
    };
    rx.wait_for(|v| v.is_some())
        .await
        .ok()
        .and_then(|v| *v)
        .unwrap_or(false)
}

async fn index_job(inner: &Arc<Inner>, job: &PhotoJob) -> Result<()> {
    let source = blob_path(&inner.root, &job.blob_id)?;
    let mut facts = {
        let _permit = inner.slots.acquire().await.map_err(|_| AppError::internal())?;
        read_facts(&source, &job.kind, job.modified_at, &job.name, &job.media_type).await
    };
    let ok = run_thumbnail(inner, &job.blob_id, &job.kind, "s").await;
    if ok
        && let Ok(thumb) = cache_path(&inner.cache, &job.blob_id, "s")
        && let Ok(_permit) = inner.slots.acquire().await
    {
        facts.phash = perceptual_hash(&thumb).await;
    }
    facts.sha1 = sha1_of(&source).await.unwrap_or_default();
    let payload = json!({
        "fileId": job.file_id,
        "blobId": job.blob_id,
        "facts": facts,
        "thumbnail": if ok { "ready" } else { "failed" },
    });
    inner
        .db
        .call("service_photos_worker", json!(["store", payload.to_string()]))
        .await?;
    Ok(())
}

async fn scan_pass(inner: &Arc<Inner>) -> Result<()> {
    for _ in 0..50 {
        if inner.closing.load(Ordering::Relaxed) {
            return Ok(());
        }
        let arg = json!({ "limit": 5000, "jobs": 8 }).to_string();
        let res = inner
            .db
            .call("service_photos_worker", json!(["scan", arg]))
            .await?;
        let jobs: Vec<PhotoJob> = res
            .get("jobs")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let done = res.get("done").and_then(Value::as_bool).unwrap_or(true);
        let empty = jobs.is_empty();
        for job in jobs {
            if inner.closing.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Err(err) = index_job(inner, &job).await {
                eprintln!("Photos indexing skipped a file: {err}");
            }
        }
        if done && empty {
            return Ok(());
        }
    }
    Ok(())
}

async fn scan_loop(inner: Arc<Inner>) {
    let mut delay = Duration::from_secs(2);
    loop {
        if inner.closing.load(Ordering::Relaxed) {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = inner.scan_notify.notified() => {}
        }
        if inner.closing.load(Ordering::Relaxed) {
            break;
        }
        {
            let mut state = inner.scan_state.lock().expect("scan_state");
            state.0 = true;
            state.1 = now_ms();
        }
        if let Err(err) = scan_pass(&inner).await
            && !inner.closing.load(Ordering::Relaxed)
        {
            eprintln!("Photos scan failed: {err}");
        }
        {
            let mut state = inner.scan_state.lock().expect("scan_state");
            state.0 = false;
        }
        delay = Duration::from_secs(30);
    }
}

async fn video_loop(inner: Arc<Inner>, cancel_rx: watch::Receiver<bool>) {
    loop {
        if inner.closing.load(Ordering::Relaxed) {
            break;
        }
        let next = inner.video_queue.lock().expect("video_queue").pop_front();
        let Some(request) = next else {
            inner.video_notify.notified().await;
            continue;
        };
        if inner.closing.load(Ordering::Relaxed) {
            break;
        }
        let Ok(source) = blob_path(&inner.root, &request.blob_id) else {
            inner.videos.lock().expect("videos").insert(
                request.blob_id,
                VideoEntry { state: "failed", progress: 0.0, until: Some(now_ms() + 600_000.0) },
            );
            continue;
        };
        let blob_id = request.blob_id.clone();
        let progress_inner = inner.clone();
        let ok = make_video(
            &inner.cache,
            &source,
            &request.blob_id,
            &request.mode,
            &request.audio,
            request.duration,
            move |fraction| {
                if let Some(entry) = progress_inner.videos.lock().expect("videos").get_mut(&blob_id) {
                    entry.progress = fraction;
                }
            },
            cancel_rx.clone(),
        )
        .await;
        let mut videos = inner.videos.lock().expect("videos");
        if ok {
            videos.remove(&request.blob_id);
        } else {
            videos.insert(
                request.blob_id,
                VideoEntry { state: "failed", progress: 0.0, until: Some(now_ms() + 600_000.0) },
            );
        }
    }
}

/// Base64 SHA-1 of a file, as Immich clients send checksums.
async fn sha1_of(file: &Path) -> std::io::Result<String> {
    let mut f = tokio::fs::File::open(file).await?;
    let mut hash = Sha1::default();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(sha1_hex_to_base64(&hash.hex()))
}
