//! Chat attachments (`next/runtime/chat/media.ts`): the bridge decrypts each into the
//! runtime's download directory, then it joins the blob store under its digest and the
//! database records it, which keeps it through maintenance and puts it in backups.
//! One download per attachment at a time; automatic ones run two at once.

use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, Method};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_util::io::ReaderStream;

use crate::blobs::{BlobFile, Blobs};
use crate::db::Database;
use crate::error::{AppError, Result};

use super::bridge::Bridge;
use super::{CHAT_READ_OPERATIONS, local_name};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaRow {
    id: String,
    #[allow(dead_code)]
    account_id: String,
    #[allow(dead_code)]
    chat: String,
    #[allow(dead_code)]
    message_id: String,
    kind: String,
    mime: String,
    size: f64,
    file_name: String,
    blob_id: String,
    state: String,
    at: f64,
    #[allow(dead_code)]
    chat_name: String,
    #[serde(default)]
    platform: String,
}

struct MediaState {
    running: HashMap<String, watch::Receiver<Option<Result<()>>>>,
    queue: VecDeque<String>,
    active: usize,
    closing: bool,
}

struct MediaInner {
    db: Database,
    downloads: PathBuf,
    blobs: Blobs,
    bridge: Bridge,
    state: Mutex<MediaState>,
}

#[derive(Clone)]
pub(crate) struct ChatMedia {
    inner: Arc<MediaInner>,
}

fn state_error(code: &str) -> Option<(u16, &'static str)> {
    match code {
        "retrying" => Some((
            409,
            "WhatsApp no longer has this file. Your phone was asked to send it again; try again in a moment.",
        )),
        "expired" => Some((
            410,
            "This file is no longer available from the service, and could not be fetched again.",
        )),
        "failed" => Some((502, "The attachment could not be downloaded. Try again.")),
        _ => None,
    }
}

impl ChatMedia {
    pub async fn open(
        db: Database,
        root: &Path,
        blobs: Blobs,
        bridge: Bridge,
    ) -> std::io::Result<Self> {
        let downloads = root.join("chat-downloads");
        tokio::fs::create_dir_all(&downloads).await?;
        tokio::fs::set_permissions(&downloads, std::fs::Permissions::from_mode(0o700)).await?;
        // A download interrupted by a stop leaves a partial file nothing refers to.
        let mut entries = tokio::fs::read_dir(&downloads).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.ends_with(".part"))
            {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
        Ok(Self {
            inner: Arc::new(MediaInner {
                db,
                downloads,
                blobs,
                bridge,
                state: Mutex::new(MediaState {
                    running: HashMap::new(),
                    queue: VecDeque::new(),
                    active: 0,
                    closing: false,
                }),
            }),
        })
    }

    async fn worker(&self, operation: &str, value: Value) -> Result<Value> {
        self.inner
            .db
            .operation(
                "service_chat_worker",
                operation,
                CHAT_READ_OPERATIONS,
                json!([operation, value.to_string()]),
            )
            .await
    }

    async fn call(&self, session_id: &str, operation: &str, value: Value) -> Result<Value> {
        self.inner
            .db
            .operation(
                "service_chat",
                operation,
                CHAT_READ_OPERATIONS,
                json!([session_id, operation, value.to_string()]),
            )
            .await
    }

    /// Downloads one attachment, identified by its message row ID `"<account>/<chat>/<message>"`.
    /// Concurrent callers for the same ID share a single bridge download.
    async fn fetch(&self, id: String) -> Result<()> {
        let mut rx = {
            let mut state = self.inner.state.lock().expect("media state");
            if let Some(existing) = state.running.get(&id) {
                existing.clone()
            } else {
                let (tx, rx) = watch::channel(None);
                state.running.insert(id.clone(), rx.clone());
                let media = self.clone();
                let id_for_task = id.clone();
                tokio::spawn(async move {
                    let result = media.run_download(&id_for_task).await;
                    let _ = tx.send(Some(result));
                    media
                        .inner
                        .state
                        .lock()
                        .expect("media state")
                        .running
                        .remove(&id_for_task);
                });
                rx
            }
        };

        let value = rx
            .wait_for(|v| v.is_some())
            .await
            .map_err(|_| AppError::with("failed", 502, "The attachment could not be downloaded. Try again."))?;
        (*value).clone().expect("download result")
    }

    async fn run_download(&self, id: &str) -> Result<()> {
        let mut parts = id.splitn(3, '/');
        let account_id = parts.next().unwrap_or_default();
        let chat = parts.next().unwrap_or_default();
        let message_id = parts.next().unwrap_or_default();

        let file = self
            .inner
            .downloads
            .join(format!("{}.part", crate::auth::uuid()));
        let file_str = file.to_string_lossy().into_owned();

        let outcome: Result<()> = async {
            self.inner
                .bridge
                .call(
                    "download",
                    json!({
                        "account": account_id,
                        "chat": chat,
                        "id": message_id,
                        "path": file_str,
                    }),
                    Duration::from_secs(15 * 60),
                )
                .await?;
            let meta = tokio::fs::metadata(&file)
                .await
                .map_err(|_| AppError::internal())?;
            let size = meta.len();
            let opened = tokio::fs::File::open(&file)
                .await
                .map_err(|_| AppError::internal())?;
            let blob_id = self
                .inner
                .blobs
                .receive(ReaderStream::new(opened), size)
                .await?;
            self.worker(
                "mediaStored",
                json!({
                    "accountId": account_id,
                    "id": id,
                    "blobId": blob_id,
                    "size": size,
                }),
            )
            .await?;
            Ok(())
        }
        .await;

        let _ = tokio::fs::remove_file(&file).await;

        if let Err(ref error) = outcome {
            let closing = self.inner.state.lock().expect("media state").closing;
            if !closing {
                let next_state = match error.code.as_str() {
                    "retrying" => "retrying",
                    "expired" | "noMedia" => "expired",
                    _ => "failed",
                };
                let _ = self
                    .worker(
                        "mediaState",
                        json!({
                            "accountId": account_id,
                            "id": id,
                            "state": next_state,
                        }),
                    )
                    .await;
            }
        }
        outcome
    }

    fn pump(&self) {
        let mut to_spawn = Vec::new();
        {
            let mut state = self.inner.state.lock().expect("media state");
            while state.active < 2 && !state.closing {
                let Some(id) = state.queue.pop_front() else {
                    break;
                };
                state.active += 1;
                to_spawn.push(id);
            }
        }
        for id in to_spawn {
            let media = self.clone();
            tokio::spawn(async move {
                let _ = media.fetch(id).await;
                {
                    let mut state = media.inner.state.lock().expect("media state");
                    state.active = state.active.saturating_sub(1);
                }
                media.pump();
            });
        }
    }

    async fn ready(&self, session_id: &str, thread_id: &str, message_id: &str) -> Result<MediaRow> {
        let mut row: MediaRow = serde_json::from_value(
            self.call(
                session_id,
                "media",
                json!({ "threadId": thread_id, "messageId": message_id }),
            )
            .await?,
        )
        .map_err(|_| AppError::internal())?;
        if row.state == "stored" {
            return Ok(row);
        }
        if let Err(error) = self.fetch(row.id.clone()).await {
            if let Some((status, msg)) = state_error(&error.code) {
                return Err(AppError::with(&error.code, status, msg));
            }
            if error.code == "notConnected" {
                let name = if row.platform == "discord" {
                    "Discord"
                } else {
                    "WhatsApp"
                };
                return Err(AppError::with(
                    "notConnected",
                    409,
                    &format!("{name} is not connected, so the attachment cannot be downloaded now."),
                ));
            }
            let (status, msg) = state_error("failed").expect("failed error");
            return Err(AppError::with("failed", status, msg));
        }
        row = serde_json::from_value(
            self.call(
                session_id,
                "media",
                json!({ "threadId": thread_id, "messageId": message_id }),
            )
            .await?,
        )
        .map_err(|_| AppError::internal())?;
        if row.state != "stored" {
            let (status, msg) = state_error("failed").expect("failed error");
            return Err(AppError::with("failed", status, msg));
        }
        Ok(row)
    }

    /// New incoming photos, stickers and voice notes.
    pub fn queue(&self, ids: Vec<String>) {
        {
            let mut state = self.inner.state.lock().expect("media state");
            for id in ids {
                if !state.queue.contains(&id) {
                    state.queue.push_back(id);
                }
            }
        }
        self.pump();
    }

    /// The phone sent expired media again.
    pub async fn retried(
        &self,
        account_id: &str,
        chat: &str,
        message_id: &str,
        ok: bool,
    ) -> Result<()> {
        let found = self
            .worker(
                "mediaFind",
                json!({
                    "accountId": account_id,
                    "chat": chat,
                    "messageId": message_id,
                }),
            )
            .await?;
        let id = found["id"].as_str().unwrap_or_default();
        if id.is_empty() {
            return Ok(());
        }
        if ok {
            self.queue(vec![id.to_owned()]);
        } else {
            self.worker(
                "mediaState",
                json!({
                    "accountId": account_id,
                    "id": id,
                    "state": "expired",
                }),
            )
            .await?;
        }
        Ok(())
    }

    /// Serves an attachment, downloading it first if needed; HTML and other active types always download.
    pub async fn serve(
        &self,
        method: &Method,
        headers: &HeaderMap,
        session_id: &str,
        thread_id: &str,
        message_id: &str,
        attachment: bool,
    ) -> Result<Response> {
        let row = self.ready(session_id, thread_id, message_id).await?;
        let entry = BlobFile {
            id: row.id,
            name: local_name(&row.kind, &row.mime, &row.file_name, row.at, &row.platform),
            blob_id: row.blob_id,
            size: row.size,
            media_type: row.mime,
            revision: 0,
            locked: false,
        };
        self.inner
            .blobs
            .download(method, headers, &entry, !attachment)
            .await
    }

    /// Copies an attachment into a Files folder as a new file, numbering the name if it is taken.
    pub async fn save(
        &self,
        session_id: &str,
        thread_id: &str,
        message_id: &str,
        folder_id: &str,
    ) -> Result<Value> {
        let row = self.ready(session_id, thread_id, message_id).await?;
        let name = local_name(&row.kind, &row.mime, &row.file_name, row.at, &row.platform);
        let (stem, extension) = match name.rfind('.') {
            Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
            _ => (name.as_str(), ""),
        };
        for attempt in 1..=50 {
            let candidate = if attempt == 1 {
                name.clone()
            } else {
                format!("{stem} ({attempt}){extension}")
            };
            let upload_id = crate::auth::uuid();
            let file_id = crate::auth::uuid();
            match self
                .inner
                .db
                .call(
                    "service_begin_upload",
                    json!([
                        session_id,
                        upload_id,
                        file_id,
                        folder_id,
                        candidate,
                        row.size,
                        row.mime,
                        0,
                    ]),
                )
                .await
            {
                Ok(_) => {}
                Err(err) if err.code == "conflict" => continue,
                Err(err) => return Err(err),
            }
            let blob_path = self.inner.blobs.path(&row.blob_id)?;
            let file = tokio::fs::File::open(&blob_path)
                .await
                .map_err(|_| AppError::new("contentUnavailable", 503))?;
            let digest = self
                .inner
                .blobs
                .receive(ReaderStream::new(file), row.size as u64)
                .await?;
            self.inner
                .db
                .call(
                    "service_finish_upload",
                    json!([session_id, upload_id, digest, 0]),
                )
                .await?;
            return Ok(json!({ "fileId": file_id, "name": candidate }));
        }
        Err(AppError::with(
            "conflict",
            409,
            "Too many files with this name in that folder.",
        ))
    }

    pub fn close(&self) {
        let mut state = self.inner.state.lock().expect("media state");
        state.closing = true;
        state.queue.clear();
    }
}
