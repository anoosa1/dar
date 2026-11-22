//! Immutable local content named by its SHA-256 (`runtime/files/blobs.ts`): a receive hashes
//! into a private temporary file, then links it under its digest or keeps the identical
//! content already stored. Downloads support one byte range and conditional requests.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::Response;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::error::{AppError, Result};

pub fn is_blob_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A downloadable file as the module describes it (`service_download`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobFile {
    pub id: String,
    pub name: String,
    pub blob_id: String,
    pub size: f64,
    pub media_type: String,
    pub revision: u32,
    /// Inside the Locked folder: never kept by browser caches.
    #[serde(default)]
    pub locked: bool,
}

/// Content linked or used within this long is never removed: its upload may be about to record it.
pub const GRACE_MS: u128 = 2 * 3_600_000;

#[derive(Clone)]
pub struct Blobs {
    directory: PathBuf,
    uploads: PathBuf,
    grace_ms: u128,
    /// Storing content under a digest and removing that digest never overlap.
    turns: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>>,
}

impl Blobs {
    pub fn open(root: &Path) -> std::io::Result<Blobs> {
        let directory = root.join("blobs");
        let uploads = root.join("uploads");
        std::fs::create_dir_all(&directory)?;
        std::fs::create_dir_all(&uploads)?;
        Ok(Blobs { directory, uploads, grace_ms: GRACE_MS, turns: Default::default() })
    }

    async fn turn(&self, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut turns = self.turns.lock().expect("turns");
            turns.retain(|_, l| std::sync::Arc::strong_count(l) > 1);
            turns.entry(id.to_owned()).or_default().clone()
        };
        lock.lock_owned().await
    }

    async fn sync_directory(&self) {
        if let Ok(dir) = tokio::fs::File::open(&self.directory).await {
            let _ = dir.sync_all().await;
        }
    }

    /// Removes temporary upload files a stopped runtime left behind (`recover`).
    pub async fn recover(&self) -> std::io::Result<()> {
        let mut entries = tokio::fs::read_dir(&self.uploads).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let ok = name.strip_suffix(".tmp").is_some_and(|id| id.len() == 36 && id.bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-')));
            if !ok {
                return Err(std::io::Error::other("Unrecognized temporary upload file."));
            }
            tokio::fs::remove_file(entry.path()).await?;
        }
        Ok(())
    }

    fn settled_enough(&self, meta: &std::fs::Metadata) -> bool {
        let age = meta.modified().ok().and_then(|m| m.elapsed().ok()).map_or(0, |d| d.as_millis());
        age >= self.grace_ms
    }

    /// Removes content nothing linked or used within the grace period; false when it was kept.
    pub async fn remove(&self, id: &str) -> Result<bool> {
        let target = self.path(id)?;
        let _turn = self.turn(id).await;
        let meta = match tokio::fs::symlink_metadata(&target).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(_) => return Err(AppError::internal()),
        };
        if !meta.is_file() {
            return Err(AppError::internal());
        }
        if !self.settled_enough(&meta) {
            return Ok(false);
        }
        tokio::fs::remove_file(&target).await.map_err(|_| AppError::internal())?;
        self.sync_directory().await;
        Ok(true)
    }

    /// Stored content past its grace period, by name (for the orphan sweep).
    pub async fn settled(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(&self.directory).await else { return out };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_blob_id(&name) {
                continue;
            }
            if let Ok(meta) = tokio::fs::symlink_metadata(entry.path()).await
                && meta.is_file()
                && self.settled_enough(&meta)
            {
                out.push(name);
            }
        }
        out
    }

    pub fn path(&self, id: &str) -> Result<PathBuf> {
        if !is_blob_id(id) {
            return Err(AppError::new("invalidBlobId", 400));
        }
        Ok(self.directory.join(id))
    }

    /// Stores exactly `size` bytes from `source`; returns their digest.
    pub async fn receive<S, E>(&self, source: S, size: u64) -> Result<String>
    where
        S: Stream<Item = std::result::Result<axum::body::Bytes, E>> + Unpin,
    {
        self.receive_named(source, size, None).await
    }

    /// `receive`, with the temporary file named after an upload reservation (`uploads/<id>.tmp`,
    /// as the TypeScript runtime named it), so an interrupted upload is identifiable on disk.
    pub async fn receive_named<S, E>(&self, mut source: S, size: u64, name: Option<&str>) -> Result<String>
    where
        S: Stream<Item = std::result::Result<axum::body::Bytes, E>> + Unpin,
    {
        let temporary = self.uploads.join(format!("{}.tmp", name.map_or_else(crate::auth::uuid, str::to_owned)));
        let result = async {
            let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary).await.map_err(|_| AppError::internal())?;
            let (mut hash, mut written) = (Sha256::new(), 0u64);
            while let Some(chunk) = source.next().await {
                let chunk = chunk.map_err(|_| AppError::with("incompleteUpload", 400, "The upload was interrupted."))?;
                written += chunk.len() as u64;
                if written > size {
                    return Err(AppError::new("uploadTooLarge", 413));
                }
                hash.update(&chunk);
                file.write_all(&chunk).await.map_err(|_| AppError::internal())?;
            }
            if written != size {
                return Err(AppError::new("incompleteUpload", 400));
            }
            file.sync_all().await.map_err(|_| AppError::internal())?;
            let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
            let target = self.path(&digest)?;
            let _turn = self.turn(&digest).await;
            // Identical content already stored is kept and marked used now; a new name is linked and its directory flushed.
            match tokio::fs::hard_link(&temporary, &target).await {
                Ok(()) => self.sync_directory().await,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let existing = std::fs::OpenOptions::new().read(true).open(&target).map_err(|_| AppError::new("contentUnavailable", 503))?;
                    let meta = existing.metadata().map_err(|_| AppError::new("contentUnavailable", 503))?;
                    if !meta.is_file() || meta.len() != size {
                        return Err(AppError::new("contentUnavailable", 503));
                    }
                    let now = std::time::SystemTime::now();
                    existing.set_times(std::fs::FileTimes::new().set_accessed(now).set_modified(now)).map_err(|_| AppError::internal())?;
                }
                Err(_) => return Err(AppError::internal()),
            }
            Ok(digest)
        }
        .await;
        let _ = tokio::fs::remove_file(&temporary).await;
        result
    }

    /// Stores an in-memory buffer as an immutable blob; returns its SHA-256 digest.
    pub async fn store_bytes(&self, data: impl Into<axum::body::Bytes>) -> Result<String> {
        let bytes = data.into();
        let len = bytes.len() as u64;
        let stream = futures_util::stream::once(async move { Ok::<axum::body::Bytes, std::io::Error>(bytes) });
        self.receive(Box::pin(stream), len).await
    }

    /// Sends a file: attachments by default, inline only for types browsers render safely.
    pub async fn download(&self, method: &Method, headers: &HeaderMap, entry: &BlobFile, preview: bool) -> Result<Response> {
        let path = self.path(&entry.blob_id)?;
        let mut file = tokio::fs::File::open(&path).await.map_err(|_| AppError::new("contentUnavailable", 503))?;
        let size = entry.size as u64;
        let meta = file.metadata().await.map_err(|_| AppError::new("contentUnavailable", 503))?;
        if !meta.is_file() || meta.len() != size {
            return Err(AppError::new("contentUnavailable", 503));
        }
        let etag = format!("\"{}:{}:{}\"", entry.id, entry.revision, entry.blob_id);
        let inline_types = [
            "image/png", "image/jpeg", "image/gif", "image/webp", "image/avif", "text/plain", "application/pdf", "video/mp4", "video/webm", "video/ogg",
            "audio/mpeg", "audio/mp4", "audio/ogg", "audio/wav", "audio/webm", "audio/flac", "audio/aac", "audio/x-wav", "audio/x-flac",
        ];
        let inline = preview && inline_types.contains(&entry.media_type.as_str());
        let mut response = Response::builder()
            .header(header::ETAG, &etag)
            .header(header::CACHE_CONTROL, "private, no-store")
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_TYPE, if inline { entry.media_type.as_str() } else { "application/octet-stream" })
            .header(header::CONTENT_DISPOSITION, format!("{}; filename*=UTF-8''{}", if inline { "inline" } else { "attachment" }, encode_filename(&entry.name)));
        if inline {
            response = response.header(header::CONTENT_SECURITY_POLICY, "sandbox; default-src 'none'; frame-ancestors 'self'");
        }
        if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
            return Ok(response.status(StatusCode::NOT_MODIFIED).body(Body::empty()).expect("response"));
        }
        let (mut start, mut end, mut status) = (0u64, size.saturating_sub(1), StatusCode::OK);
        let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
        let if_range = headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok());
        if let Some(range) = range.filter(|_| if_range.is_none_or(|r| r == etag)) {
            let invalid = || AppError::new("invalidRange", 416);
            let spec = range.strip_prefix("bytes=").ok_or_else(invalid)?;
            let (from, to) = spec.split_once('-').ok_or_else(invalid)?;
            if from.is_empty() && to.is_empty() || size == 0 {
                return Err(invalid());
            }
            if from.is_empty() {
                let suffix: u64 = to.parse().map_err(|_| invalid())?;
                if suffix == 0 {
                    return Err(invalid());
                }
                start = size.saturating_sub(suffix);
            } else {
                start = from.parse().map_err(|_| invalid())?;
                if !to.is_empty() {
                    end = end.min(to.parse().map_err(|_| invalid())?);
                }
            }
            if start > end || start >= size {
                return Err(invalid());
            }
            status = StatusCode::PARTIAL_CONTENT;
            response = response.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
        }
        let length = if size == 0 { 0 } else { end - start + 1 };
        response = response.status(status).header(header::CONTENT_LENGTH, length);
        if method == Method::HEAD || size == 0 {
            return Ok(response.body(Body::empty()).expect("response"));
        }
        file.seek(std::io::SeekFrom::Start(start)).await.map_err(|_| AppError::internal())?;
        let stream = tokio_util::io::ReaderStream::new(tokio::io::AsyncReadExt::take(file, length));
        Ok(response.body(Body::from_stream(stream)).expect("response"))
    }
}

/// `encodeURIComponent`, then the characters it leaves that RFC 5987 does not allow.
fn encode_filename(name: &str) -> String {
    let mut out = String::new();
    for byte in name.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn receives_by_digest_and_rejects_wrong_sizes() {
        let root = std::env::temp_dir().join(format!("dar-blobs-{}", crate::auth::uuid()));
        let blobs = Blobs::open(&root).unwrap();
        let chunks = || futures_util::stream::iter(vec![Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"hel")), Ok(axum::body::Bytes::from_static(b"lo"))]);
        let digest = blobs.receive(chunks(), 5).await.unwrap();
        assert_eq!(digest, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        assert_eq!(blobs.receive(chunks(), 5).await.unwrap(), digest, "identical content is kept");
        assert_eq!(blobs.receive(chunks(), 4).await.unwrap_err().status, 413);
        assert_eq!(blobs.receive(chunks(), 6).await.unwrap_err().status, 400);
        assert_eq!(std::fs::read_dir(root.join("uploads")).unwrap().count(), 0, "temporary files are removed");
        // Content used again within the grace period stays; settled content goes.
        assert!(!blobs.remove(&digest).await.unwrap());
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 3600);
        std::fs::File::options().write(true).open(blobs.path(&digest).unwrap()).unwrap().set_modified(old).unwrap();
        assert_eq!(blobs.settled().await, vec![digest.clone()]);
        assert!(blobs.remove(&digest).await.unwrap());
        assert!(blobs.settled().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filenames_encode_like_the_node_runtime() {
        assert_eq!(encode_filename("a b(1)'.txt"), "a%20b%281%29%27.txt");
    }
}
