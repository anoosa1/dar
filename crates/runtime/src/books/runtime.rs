//! Books' derived data runtime (`next/runtime/books/runtime.ts`), all regenerable and keyed by
//! immutable blob ID under `book-cache/`: parsed metadata (memory and disk), WebP cover thumbnails,
//! cached open ZIP handles, and unpacked non-ZIP comic archives (`bsdtar`). External tool runs are
//! bounded to two concurrent slots with minimal environment, strict timeouts, and output caps.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::future::{FutureExt, Shared, join_all};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;

use crate::blobs::is_blob_id;
use crate::db::Database;
use crate::error::{AppError, Result};

use super::formats::{
    BookFormat, BookMeta, BookTocEntry, SpineItem, book_format, book_position, comic_pages, empty_meta,
    read_comic_info, read_epub, truncate_chars,
};
use super::pdf::{PDF_COVER_MAX_PX, PdfInfoOutcome, configure_child_limits, parse_info_output};
use super::zip::{DEFAULT_MAX_ENTRIES, Zip, ZipError};

#[cfg(target_arch = "aarch64")]
const O_NOFOLLOW: i32 = 0o100000;
#[cfg(not(target_arch = "aarch64"))]
const O_NOFOLLOW: i32 = 0o400000;

const INFO_VERSION: u32 = 1;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type SharedFuture<T> = Shared<BoxFuture<T>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpubInfo {
    pub opf: String,
    pub spine: Vec<SpineItem>,
    pub toc: Vec<BookTocEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    pub meta: BookMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epub: Option<EpubInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
struct SavedInfo {
    version: u32,
    info: Info,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryListing {
    root_id: String,
    path: String,
    libraries: Vec<LibraryFolder>,
    files: Vec<LibraryFile>,
    states: Vec<Value>,
    truncated: bool,
}

#[derive(Deserialize)]
struct LibraryFolder {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryFile {
    id: String,
    parent_id: String,
    name: String,
    blob_id: String,
    size: Value,
    revision: Value,
    modified_at: Value,
    path: String,
    library: String,
}

struct OpenZip {
    zip: SharedFuture<std::result::Result<Arc<Zip>, ZipError>>,
    used: Instant,
}

struct InfoCache {
    map: HashMap<String, SharedFuture<Arc<Info>>>,
    order: VecDeque<String>,
}

impl InfoCache {
    fn new() -> Self {
        Self { map: HashMap::new(), order: VecDeque::new() }
    }
}

struct Inner {
    db: Database,
    root: PathBuf,
    cache: PathBuf,
    slots: Semaphore,
    infos: Mutex<InfoCache>,
    unpacks: Mutex<HashMap<String, SharedFuture<std::result::Result<PathBuf, String>>>>,
    covers: Mutex<HashMap<String, SharedFuture<Option<PathBuf>>>>,
    zips: Mutex<HashMap<String, OpenZip>>,
    shutdown_tx: watch::Sender<bool>,
    sweep_handle: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct BooksRuntime {
    inner: Arc<Inner>,
}

struct RunOutput {
    stdout: String,
    ok: bool,
}

fn tool_path(name: &str) -> String {
    std::env::var(format!("DAR_{}", name.to_ascii_uppercase()))
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| name.to_owned())
}

async fn run_command(mut cmd: Command, timeout: Duration, max_buffer: usize) -> RunOutput {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let Ok(mut child) = cmd.spawn() else {
        return RunOutput { stdout: String::new(), ok: false };
    };

    let work = async {
        let mut buf = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            let mut limited = stdout.take((max_buffer as u64).saturating_add(1));
            if limited.read_to_end(&mut buf).await.is_err() {
                let _ = child.kill().await;
                return RunOutput { stdout: String::new(), ok: false };
            }
            if buf.len() > max_buffer {
                let _ = child.kill().await;
                return RunOutput { stdout: String::new(), ok: false };
            }
        }
        let ok = child.wait().await.is_ok_and(|s| s.success());
        RunOutput { stdout: String::from_utf8_lossy(&buf).into_owned(), ok }
    };

    match tokio::time::timeout(timeout, work).await {
        Ok(out) => out,
        Err(_) => RunOutput { stdout: String::new(), ok: false },
    }
}

async fn run_tool(name: &str, args: &[String], timeout: Duration, max_buffer: usize) -> RunOutput {
    let mut cmd = Command::new(tool_path(name));
    cmd.args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", "/nonexistent")
        .env("LC_ALL", "C.UTF-8");
    run_command(cmd, timeout, max_buffer).await
}

async fn run_pdf_helper(args: &[String], timeout: Duration, max_buffer: usize) -> RunOutput {
    let Ok(exe) = std::env::current_exe() else {
        return RunOutput { stdout: String::new(), ok: false };
    };
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env_clear()
        .env("HOME", "/nonexistent")
        .env("LC_ALL", "C.UTF-8");
    configure_child_limits(&mut cmd);
    run_command(cmd, timeout, max_buffer).await
}

async fn exists_nonempty(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok_and(|m| m.len() > 0)
}

async fn is_dir_nofollow(path: &Path) -> bool {
    tokio::fs::symlink_metadata(path).await.is_ok_and(|m| m.is_dir())
}

async fn mkdir_private(path: &Path) -> std::io::Result<()> {
    tokio::fs::DirBuilder::new().recursive(true).mode(0o700).create(path).await
}

async fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await?;
    file.write_all(bytes).await?;
    file.flush().await
}

fn extension_lower(name: &str) -> Option<String> {
    let (_, ext) = name.rsplit_once('.')?;
    if !ext.is_empty() && ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Some(ext.to_ascii_lowercase())
    } else {
        None
    }
}

pub fn image_type(name: &str) -> Option<&'static str> {
    match extension_lower(name)?.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Types an EPUB may hold; others are served as plain bytes the browser will not run.
pub fn epub_type(name: &str) -> &'static str {
    if let Some(mime) = image_type(name) {
        return mime;
    }
    match extension_lower(name).as_deref() {
        Some("xhtml" | "html" | "htm") => "application/xhtml+xml",
        Some("xml" | "ncx" | "opf") => "application/xml",
        Some("css") => "text/css",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("mp3") => "audio/mpeg",
        Some("mp4" | "m4a") => "audio/mp4",
        _ => "application/octet-stream",
    }
}

fn strip_ext(name: &str) -> &str {
    match name.rfind('.') {
        Some(idx) if idx + 1 < name.len() => &name[..idx],
        _ => name,
    }
}

fn fallback_title(name: &str) -> String {
    static UNDERSCORES: OnceLock<Regex> = OnceLock::new();
    let stripped = strip_ext(name);
    let cleaned = UNDERSCORES.get_or_init(|| Regex::new(r"_+").expect("regex")).replace_all(stripped, " ");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() { name.to_owned() } else { trimmed.to_owned() }
}

fn cover_ext(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < base.len() => {
            let ext = &base[dot + 1..];
            if ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
                format!(".{}", ext.to_ascii_lowercase())
            } else {
                ".img".into()
            }
        }
        _ => ".img".into(),
    }
}

async fn list_files(directory: &Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(prefix) = stack.pop() {
        let dir_path = if prefix.is_empty() { directory.to_path_buf() } else { directory.join(&prefix) };
        let mut entries = tokio::fs::read_dir(&dir_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let name = if prefix.is_empty() { file_name } else { format!("{prefix}/{file_name}") };
            let ft = entry.file_type().await?;
            if ft.is_dir() && !ft.is_symlink() {
                stack.push(name);
            } else if ft.is_file() {
                out.push(name);
            }
            if out.len() > 20_000 {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

async fn read_page_file(directory: &Path, name: &str, max: usize) -> std::result::Result<Vec<u8>, String> {
    let rel = Path::new(name);
    if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("Invalid page path.".into());
    }
    let full = directory.join(rel);
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(&full)
            .map_err(|_| "Missing page.".to_owned())?;
        let meta = file.metadata().map_err(|_| "Missing page.".to_owned())?;
        if !meta.is_file() {
            return Err("Missing page.".into());
        }
        if meta.len() > max as u64 {
            return Err("Page too large.".into());
        }
        let mut out = Vec::with_capacity(meta.len() as usize);
        file.read_to_end(&mut out).map_err(|_| "Unreadable page.".to_owned())?;
        Ok(out)
    })
    .await
    .map_err(|_| "Unreadable page.".to_owned())?
}

impl BooksRuntime {
    pub async fn open(db: Database, root: &Path) -> std::io::Result<Self> {
        let cache = root.join("book-cache");
        mkdir_private(&cache).await?;
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let inner = Arc::new(Inner {
            db,
            root: root.to_path_buf(),
            cache,
            slots: Semaphore::new(2),
            infos: Mutex::new(InfoCache::new()),
            unpacks: Mutex::new(HashMap::new()),
            covers: Mutex::new(HashMap::new()),
            zips: Mutex::new(HashMap::new()),
            shutdown_tx,
            sweep_handle: tokio::sync::Mutex::new(None),
        });

        let sweep_inner = Arc::downgrade(&inner);
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(30));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await; // skip immediate tick
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let Some(inner) = sweep_inner.upgrade() else { break };
                        let mut zips = inner.zips.lock().expect("zips");
                        zips.retain(|_, open| open.used.elapsed() <= Duration::from_secs(60));
                    }
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });
        *inner.sweep_handle.lock().await = Some(handle);

        Ok(Self { inner })
    }

    pub async fn close(&self) {
        let _ = self.inner.shutdown_tx.send(true);
        if let Some(handle) = self.inner.sweep_handle.lock().await.take() {
            let _ = handle.await;
        }
        let unpack_futs: Vec<_> = self.inner.unpacks.lock().expect("unpacks").values().cloned().collect();
        let cover_futs: Vec<_> = self.inner.covers.lock().expect("covers").values().cloned().collect();
        join_all(unpack_futs).await;
        join_all(cover_futs).await;
        self.inner.zips.lock().expect("zips").clear();
    }

    fn cache_file(&self, blob_id: &str, suffix: &str) -> std::result::Result<PathBuf, String> {
        if !is_blob_id(blob_id) {
            return Err("Unexpected blob ID.".into());
        }
        Ok(self.inner.cache.join(&blob_id[..2]).join(format!("{blob_id}-{suffix}")))
    }

    fn blob_path(&self, blob_id: &str) -> std::result::Result<PathBuf, String> {
        if !is_blob_id(blob_id) {
            return Err("Unexpected blob ID.".into());
        }
        Ok(self.inner.root.join("blobs").join(blob_id))
    }

    async fn zip_for(&self, blob_id: &str) -> std::result::Result<Arc<Zip>, ZipError> {
        let fut = {
            let mut zips = self.inner.zips.lock().expect("zips");
            if let Some(entry) = zips.get_mut(blob_id) {
                entry.used = Instant::now();
                entry.zip.clone()
            } else {
                let path = self.blob_path(blob_id).map_err(ZipError)?;
                let inner = self.inner.clone();
                let id = blob_id.to_owned();
                let handle = tokio::spawn(async move {
                    let result = Zip::open(&path, DEFAULT_MAX_ENTRIES).await;
                    if result.is_err() {
                        inner.zips.lock().expect("zips").remove(&id);
                    }
                    result
                });
                let mapped: BoxFuture<std::result::Result<Arc<Zip>, ZipError>> = Box::pin(async move {
                    handle.await.unwrap_or_else(|_| Err(ZipError("Damaged archive.".into())))
                });
                let shared = mapped.shared();
                zips.insert(blob_id.to_owned(), OpenZip { zip: shared.clone(), used: Instant::now() });
                if zips.len() > 16
                    && let Some(oldest) = zips.iter().min_by_key(|(_, v)| v.used).map(|(k, _)| k.clone())
                {
                    zips.remove(&oldest);
                }
                shared
            }
        };
        fut.await
    }

    /// Comic archives other than ZIP are unpacked once, after checking their listed size.
    async fn unpacked(&self, blob_id: &str) -> std::result::Result<PathBuf, String> {
        let target = self.cache_file(blob_id, "pages")?;
        if is_dir_nofollow(&target).await {
            return Ok(target);
        }
        let fut = {
            let mut unpacks = self.inner.unpacks.lock().expect("unpacks");
            if let Some(existing) = unpacks.get(blob_id) {
                existing.clone()
            } else {
                let rt = self.clone();
                let id = blob_id.to_owned();
                let target_clone = target.clone();
                let handle = tokio::spawn(async move {
                    let result = rt.do_unpack(&id, &target_clone).await;
                    rt.inner.unpacks.lock().expect("unpacks").remove(&id);
                    result
                });
                let mapped: BoxFuture<std::result::Result<PathBuf, String>> = Box::pin(async move {
                    handle.await.unwrap_or_else(|_| Err("damaged".into()))
                });
                let shared = mapped.shared();
                unpacks.insert(blob_id.to_owned(), shared.clone());
                shared
            }
        };
        fut.await
    }

    async fn do_unpack(&self, blob_id: &str, target: &Path) -> std::result::Result<PathBuf, String> {
        let _permit = self.inner.slots.acquire().await.map_err(|_| "damaged".to_owned())?;
        if is_dir_nofollow(target).await {
            return Ok(target.to_path_buf());
        }
        let source = self.blob_path(blob_id)?;
        let source_str = source.to_string_lossy().into_owned();
        let listing = run_tool(
            "bsdtar",
            &["-tvf".into(), source_str.clone()],
            Duration::from_secs(120),
            32 * 1024 * 1024,
        )
        .await;
        if !listing.ok {
            return Err("unsupported".into());
        }

        static TAR_SIZE: OnceLock<Regex> = OnceLock::new();
        let tar_size = TAR_SIZE.get_or_init(|| Regex::new(r"^\S+\s+\d+\s+\S+\s+\S+\s+(\d+)\s").expect("regex"));
        let mut total = 0u64;
        let mut count = 0usize;
        for line in listing.stdout.lines() {
            if let Some(caps) = tar_size.captures(line)
                && let Some(size) = caps.get(1).and_then(|m| m.as_str().parse::<u64>().ok())
            {
                total = total.saturating_add(size);
                count += 1;
            }
        }
        if total > 4 * 1024 * 1024 * 1024 || count > 20_000 {
            return Err("unsupported".into());
        }

        if let Some(parent) = target.parent() {
            mkdir_private(parent).await.map_err(|_| "damaged".to_owned())?;
        }
        let work = self.inner.cache.join(format!(".work-{}", crate::auth::uuid()));
        mkdir_private(&work).await.map_err(|_| "damaged".to_owned())?;
        let work_str = work.to_string_lossy().into_owned();

        let extracted = run_tool(
            "bsdtar",
            &[
                "-xf".into(),
                source_str,
                "-C".into(),
                work_str,
                "--no-same-owner".into(),
                "--no-same-permissions".into(),
            ],
            Duration::from_secs(600),
            4 * 1024 * 1024,
        )
        .await;

        let result = if !extracted.ok {
            Err("damaged".into())
        } else {
            match tokio::fs::rename(&work, target).await {
                Ok(()) => Ok(target.to_path_buf()),
                Err(_) if is_dir_nofollow(target).await => Ok(target.to_path_buf()),
                Err(_) => Err("damaged".into()),
            }
        };
        let _ = tokio::fs::remove_dir_all(&work).await;
        result
    }

    async fn parse(&self, blob_id: &str, format: BookFormat, name: &str) -> Info {
        let title = fallback_title(name);
        let Ok(source) = self.blob_path(blob_id) else {
            let mut meta = empty_meta(title);
            meta.error = "damaged".into();
            return Info { meta, epub: None, pages: None };
        };

        if format == BookFormat::Pdf {
            let output = {
                let Ok(_permit) = self.inner.slots.acquire().await else {
                    let mut meta = empty_meta(title);
                    meta.error = "damaged".into();
                    return Info { meta, epub: None, pages: None };
                };
                run_pdf_helper(
                    &["pdf-info".into(), source.to_string_lossy().into_owned()],
                    Duration::from_secs(60),
                    4 * 1024 * 1024,
                )
                .await
            };
            let meta = apply_pdf_info(parse_info_output(output.ok, &output.stdout), title);
            return Info { meta, epub: None, pages: None };
        }

        if format == BookFormat::Epub {
            let result = async {
                let zip = self.zip_for(blob_id).await.map_err(|e| e.0)?;
                let get = |entry: &str| zip.get(entry).map(|e| e.size);
                let read = |entry: &str, max: usize| {
                    let zip = zip.clone();
                    let found = zip.get(entry).cloned();
                    async move {
                        let found = found.ok_or_else(|| "Missing entry.".to_owned())?;
                        zip.read(&found, max).await.map_err(|e| e.0)
                    }
                };
                let mut epub = read_epub(get, read, &title).await?;
                if epub.encrypted {
                    epub.meta.error = "drm".into();
                }
                Ok::<Info, String>(Info {
                    meta: epub.meta,
                    epub: Some(EpubInfo { opf: epub.opf, spine: epub.spine, toc: epub.toc, cover: epub.cover }),
                    pages: None,
                })
            }
            .await;
            return match result {
                Ok(info) => info,
                Err(_) => {
                    let mut meta = empty_meta(title);
                    meta.error = "damaged".into();
                    Info { meta, epub: None, pages: None }
                }
            };
        }

        // Comic formats: CBZ (ZIP) or CBR/CB7/CBT (bsdtar).
        static COMIC_INFO: OnceLock<Regex> = OnceLock::new();
        let comic_info_re = COMIC_INFO.get_or_init(|| Regex::new(r"(?i)(^|/)ComicInfo\.xml$").expect("regex"));

        let mut meta = empty_meta(title);
        let parsed = async {
            let (names, info_bytes): (Vec<String>, Option<Vec<u8>>) = if format == BookFormat::Cbz {
                let zip = self.zip_for(blob_id).await.map_err(|e| e.0)?;
                let names = zip.entries().iter().filter(|e| !e.encrypted).map(|e| e.name.clone()).collect();
                let ci = zip.entries().iter().find(|e| comic_info_re.is_match(&e.name)).cloned();
                let bytes = match ci {
                    Some(entry) => Some(zip.read(&entry, 1024 * 1024).await.map_err(|e| e.0)?),
                    None => None,
                };
                (names, bytes)
            } else {
                let dir = self.unpacked(blob_id).await?;
                let names = list_files(&dir).await.map_err(|_| "damaged".to_owned())?;
                let ci = names.iter().find(|n| comic_info_re.is_match(n)).cloned();
                let bytes = match ci {
                    Some(entry) => Some(read_page_file(&dir, &entry, 1024 * 1024).await?),
                    None => None,
                };
                (names, bytes)
            };
            Ok::<_, String>((names, info_bytes))
        }
        .await;

        match parsed {
            Ok((names, info_bytes)) => {
                let pages = comic_pages(names);
                if pages.is_empty() {
                    meta.error = "damaged".into();
                    return Info { meta, epub: None, pages: None };
                }
                if let Some(bytes) = info_bytes {
                    read_comic_info(&String::from_utf8_lossy(&bytes), &mut meta);
                }
                meta.pages = pages.len() as u64;
                meta.cover = true;
                Info { meta, epub: None, pages: Some(pages) }
            }
            Err(err) => {
                meta.error = if err == "unsupported" { "unsupported".into() } else { "damaged".into() };
                Info { meta, epub: None, pages: None }
            }
        }
    }

    fn info_future(&self, blob_id: &str, format: BookFormat, name: &str) -> SharedFuture<Arc<Info>> {
        let mut cache = self.inner.infos.lock().expect("infos");
        if let Some(existing) = cache.map.get(blob_id) {
            return existing.clone();
        }
        let rt = self.clone();
        let id = blob_id.to_owned();
        let file_name = name.to_owned();
        let fallback_name = name.to_owned();
        let handle = tokio::spawn(async move {
            let saved = rt.cache_file(&id, "info.json");
            if let Ok(ref saved_path) = saved
                && let Ok(text) = tokio::fs::read_to_string(saved_path).await
                && let Ok(parsed) = serde_json::from_str::<SavedInfo>(&text)
                && parsed.version == INFO_VERSION
            {
                return Arc::new(parsed.info);
            }
            let value = rt.parse(&id, format, &file_name).await;
            if (value.meta.error.is_empty() || value.meta.error == "drm")
                && let Ok(saved_path) = saved
                && let Some(parent) = saved_path.parent()
                && mkdir_private(parent).await.is_ok()
                && let Ok(json_bytes) = serde_json::to_vec(&SavedInfo { version: INFO_VERSION, info: value.clone() })
            {
                let tmp = parent.join(format!(".info-{}.tmp", crate::auth::uuid()));
                if write_private(&tmp, &json_bytes).await.is_ok() {
                    if tokio::fs::rename(&tmp, &saved_path).await.is_err() {
                        let _ = tokio::fs::remove_file(&tmp).await;
                    }
                } else {
                    let _ = tokio::fs::remove_file(&tmp).await;
                }
            }
            Arc::new(value)
        });
        let mapped: BoxFuture<Arc<Info>> = Box::pin(async move {
            handle.await.unwrap_or_else(|_| {
                let mut meta = empty_meta(fallback_name);
                meta.error = "damaged".into();
                Arc::new(Info { meta, epub: None, pages: None })
            })
        });
        let shared = mapped.shared();
        if cache.map.len() >= 5000 {
            while let Some(oldest) = cache.order.pop_front() {
                if cache.map.remove(&oldest).is_some() {
                    break;
                }
            }
        }
        cache.order.push_back(blob_id.to_owned());
        cache.map.insert(blob_id.to_owned(), shared.clone());
        shared
    }

    /// Metadata for a blob: memory, then the disk cache, then the file itself.
    pub async fn info(&self, blob_id: &str, format: BookFormat, name: &str) -> Arc<Info> {
        self.info_future(blob_id, format, name).await
    }

    /// A WebP cover about 480 px tall, made from the EPUB cover image, the first PDF page,
    /// or the first comic page.
    pub async fn cover(&self, blob_id: &str, format: BookFormat, name: &str) -> Option<PathBuf> {
        let target = self.cache_file(blob_id, "cover.webp").ok()?;
        if exists_nonempty(&target).await {
            return Some(target);
        }
        let book = self.info(blob_id, format, name).await;
        if !book.meta.cover {
            return None;
        }

        let fut = {
            let mut covers = self.inner.covers.lock().expect("covers");
            if let Some(existing) = covers.get(blob_id) {
                existing.clone()
            } else {
                let rt = self.clone();
                let id = blob_id.to_owned();
                let file_name = name.to_owned();
                let target_clone = target.clone();
                let handle = tokio::spawn(async move {
                    let result = rt.make_cover(&id, format, &file_name, &book, &target_clone).await;
                    rt.inner.covers.lock().expect("covers").remove(&id);
                    result
                });
                let mapped: BoxFuture<Option<PathBuf>> = Box::pin(async move { handle.await.ok().flatten() });
                let shared = mapped.shared();
                covers.insert(blob_id.to_owned(), shared.clone());
                shared
            }
        };
        fut.await
    }

    async fn make_cover(
        &self,
        blob_id: &str,
        format: BookFormat,
        name: &str,
        book: &Info,
        target: &Path,
    ) -> Option<PathBuf> {
        if exists_nonempty(target).await {
            return Some(target.to_path_buf());
        }
        if let Some(parent) = target.parent() {
            mkdir_private(parent).await.ok()?;
        }
        let work = self.inner.cache.join(format!(".work-{}", crate::auth::uuid()));
        mkdir_private(&work).await.ok()?;

        let result = async {
            let input = if format == BookFormat::Pdf {
                let source = self.blob_path(blob_id).ok()?;
                let ppm = work.join("cover.ppm");
                let rendered = {
                    let _permit = self.inner.slots.acquire().await.ok()?;
                    run_pdf_helper(
                        &[
                            "pdf-cover".into(),
                            source.to_string_lossy().into_owned(),
                            ppm.to_string_lossy().into_owned(),
                            PDF_COVER_MAX_PX.to_string(),
                        ],
                        Duration::from_secs(120),
                        4 * 1024 * 1024,
                    )
                    .await
                };
                if !rendered.ok || !exists_nonempty(&ppm).await {
                    return None;
                }
                ppm.to_string_lossy().into_owned()
            } else {
                let (bytes, ext) = if format == BookFormat::Epub
                    && let Some(cover_entry) = book.epub.as_ref().and_then(|e| e.cover.as_deref())
                {
                    let zip = self.zip_for(blob_id).await.ok()?;
                    let found = zip.get(cover_entry)?.clone();
                    let data = zip.read(&found, 32 * 1024 * 1024).await.ok()?;
                    (data, cover_ext(cover_entry))
                } else if let Some(first) = book.pages.as_ref().and_then(|p| p.first()) {
                    let (data, _) = self.page(blob_id, format, name, 0).await?;
                    (data, cover_ext(first))
                } else {
                    return None;
                };
                let staged = work.join(format!("cover{ext}"));
                write_private(&staged, &bytes).await.ok()?;
                staged.to_string_lossy().into_owned()
            };

            let out = work.join("out.webp");
            let out_arg = format!("{}[Q=78,strip]", out.to_string_lossy());
            let made = {
                let _permit = self.inner.slots.acquire().await.ok()?;
                run_tool(
                    "vipsthumbnail",
                    &[input, "--size".into(), "320x480>".into(), "-o".into(), out_arg],
                    Duration::from_secs(120),
                    4 * 1024 * 1024,
                )
                .await
            };
            if !made.ok || !exists_nonempty(&out).await {
                return None;
            }
            tokio::fs::rename(&out, target).await.ok()?;
            Some(target.to_path_buf())
        }
        .await;

        let _ = tokio::fs::remove_dir_all(&work).await;
        result
    }

    /// One comic page as stored in the archive.
    pub async fn page(
        &self,
        blob_id: &str,
        format: BookFormat,
        name: &str,
        index: usize,
    ) -> Option<(Vec<u8>, &'static str)> {
        let book = self.info(blob_id, format, name).await;
        let entry = book.pages.as_ref()?.get(index)?;
        let mime = image_type(entry).unwrap_or("application/octet-stream");
        if format == BookFormat::Cbz {
            let zip = self.zip_for(blob_id).await.ok()?;
            let found = zip.get(entry)?.clone();
            let bytes = zip.read(&found, 64 * 1024 * 1024).await.ok()?;
            Some((bytes, mime))
        } else {
            let dir = self.unpacked(blob_id).await.ok()?;
            let bytes = read_page_file(&dir, entry, 64 * 1024 * 1024).await.ok()?;
            Some((bytes, mime))
        }
    }

    /// An EPUB resource by its path inside the archive.
    pub async fn resource(&self, blob_id: &str, entry: &str) -> Option<(Vec<u8>, &'static str)> {
        let zip = self.zip_for(blob_id).await.ok()?;
        let found = zip.get(entry)?.clone();
        if found.encrypted {
            return None;
        }
        let bytes = zip.read(&found, 32 * 1024 * 1024).await.ok()?;
        Some((bytes, epub_type(entry)))
    }

    /// The library with metadata; books still being read for the first time show their file name
    /// until ready.
    pub async fn library(&self, session_id: &str) -> Result<Value> {
        let raw = self.inner.db.call("service_books", json!([session_id, "library", "{}"])).await?;
        let listing: LibraryListing = serde_json::from_value(raw).map_err(|_| AppError::internal())?;

        let mut states: HashMap<String, Value> = HashMap::with_capacity(listing.states.len());
        for mut state in listing.states {
            if let Some(obj) = state.as_object_mut()
                && let Some(file_id) = obj.get("fileId").and_then(Value::as_str).map(str::to_owned)
            {
                let normalized = obj.get("position").and_then(book_position).unwrap_or(Value::Null);
                obj.insert("position".into(), normalized);
                states.insert(file_id, state);
            }
        }

        let settled: Arc<Mutex<HashMap<usize, Arc<Info>>>> = Arc::new(Mutex::new(HashMap::new()));
        let mut waiters = Vec::with_capacity(listing.files.len());
        for (index, f) in listing.files.iter().enumerate() {
            if let Some(format) = book_format(&f.name) {
                let fut = self.info_future(&f.blob_id, format, &f.name);
                let settled = settled.clone();
                waiters.push(async move {
                    let info = fut.await;
                    settled.lock().expect("settled").insert(index, info);
                });
            }
        }
        let _ = tokio::time::timeout(Duration::from_millis(2500), join_all(waiters)).await;

        let settled = settled.lock().expect("settled");
        let mut indexing = 0usize;
        let mut books = Vec::with_capacity(listing.files.len());
        let mut counts: HashMap<String, usize> = HashMap::new();

        for (index, f) in listing.files.into_iter().enumerate() {
            let Some(format) = book_format(&f.name) else {
                continue;
            };
            let meta = match settled.get(&index) {
                Some(info) => info.meta.clone(),
                None => {
                    indexing += 1;
                    empty_meta(strip_ext(&f.name))
                }
            };
            if !f.library.is_empty() {
                *counts.entry(f.library.clone()).or_insert(0) += 1;
            }
            let mut item = match serde_json::to_value(&meta) {
                Ok(Value::Object(map)) => map,
                _ => serde_json::Map::new(),
            };
            item.insert("id".into(), json!(f.id));
            item.insert("parentId".into(), json!(f.parent_id));
            item.insert("name".into(), json!(f.name));
            item.insert("format".into(), json!(format.as_str()));
            item.insert("size".into(), f.size);
            item.insert("revision".into(), f.revision);
            item.insert("modifiedAt".into(), f.modified_at);
            item.insert("library".into(), json!(f.library));
            item.insert("path".into(), json!(f.path));
            item.insert("state".into(), states.get(&f.id).cloned().unwrap_or(Value::Null));
            books.push(Value::Object(item));
        }

        let libraries: Vec<Value> = listing
            .libraries
            .into_iter()
            .map(|l| {
                let count = counts.get(&l.id).copied().unwrap_or(0);
                json!({ "id": l.id, "name": l.name, "count": count })
            })
            .collect();

        Ok(json!({
            "rootId": listing.root_id,
            "path": listing.path,
            "libraries": libraries,
            "books": books,
            "truncated": listing.truncated,
            "indexing": indexing,
        }))
    }
}

fn apply_pdf_info(outcome: PdfInfoOutcome, fallback_title: String) -> BookMeta {
    let mut meta = empty_meta(fallback_title.clone());
    match outcome {
        PdfInfoOutcome::Ok(info) => {
            meta.pages = info.pages;
            let pdf_title = truncate_chars(info.title.trim(), 300);
            meta.title = if pdf_title.is_empty() { fallback_title } else { pdf_title };
            let author = info.author.trim();
            if !author.is_empty() {
                static AUTHOR_SPLIT: OnceLock<Regex> = OnceLock::new();
                let split = AUTHOR_SPLIT.get_or_init(|| Regex::new(r"\s*[;&]\s*|\s+and\s+").expect("regex"));
                meta.authors = split
                    .split(author)
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .take(10)
                    .map(|a| truncate_chars(a, 200))
                    .collect();
            }
            meta.cover = meta.pages > 0;
        }
        PdfInfoOutcome::Encrypted => {
            meta.error = "drm".into();
        }
        PdfInfoOutcome::Damaged => {
            meta.error = "damaged".into();
        }
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::super::pdf::PdfInfo;
    use super::*;

    #[test]
    fn apply_pdf_info_maps_metadata_and_fallback_title() {
        let meta = apply_pdf_info(
            PdfInfoOutcome::Ok(PdfInfo {
                pages: 12,
                title: "  A Fixture Paper  ".into(),
                author: "Grace Columns; Ada Page & Alan Ink and Ada Two".into(),
            }),
            "paper".into(),
        );
        assert_eq!(meta.pages, 12);
        assert_eq!(meta.title, "A Fixture Paper");
        assert_eq!(meta.authors, vec!["Grace Columns", "Ada Page", "Alan Ink", "Ada Two"]);
        assert!(meta.cover);
        assert!(meta.error.is_empty());

        let untitled = apply_pdf_info(
            PdfInfoOutcome::Ok(PdfInfo { pages: 0, title: "   ".into(), author: String::new() }),
            "My Book".into(),
        );
        assert_eq!(untitled.title, "My Book");
        assert!(!untitled.cover);
        assert!(untitled.error.is_empty());
    }

    #[test]
    fn apply_pdf_info_maps_encrypted_to_drm_and_damaged_to_damaged() {
        let drm = apply_pdf_info(PdfInfoOutcome::Encrypted, "Secret".into());
        assert_eq!(drm.error, "drm");
        assert_eq!(drm.title, "Secret");
        assert!(!drm.cover);
        assert_eq!(drm.pages, 0);

        let damaged = apply_pdf_info(PdfInfoOutcome::Damaged, "Broken".into());
        assert_eq!(damaged.error, "damaged");
        assert_eq!(damaged.title, "Broken");
        assert!(!damaged.cover);
        assert_eq!(damaged.pages, 0);
    }
}
