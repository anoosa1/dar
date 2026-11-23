//! Isolated per-thread workspaces, safe Git wrapper, bounded scanning, diffs and branch changes
//! (`next/runtime/agent/workspace.ts` and `next/runtime/files/media-types.ts`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::agent::codex::CodexConfig;
use crate::agent::{CancelToken, O_NOFOLLOW, kill_process_group, slice_utf16, utf16_len};
use crate::blobs::{BlobFile, is_blob_id};
use crate::db::Database;
use crate::error::{AppError, Result};
use crate::root::create_durable_file;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProject {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub source_type: String,
    pub source_ref: String,
    pub created_at: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    pub id: String,
    pub path: String,
    pub revision: u32,
    pub size: u64,
    pub media_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub project: AgentProject,
    pub files: Vec<SourceFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BaselineFile {
    path: String,
    content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Baseline {
    source: Source,
    files: Vec<BaselineFile>,
}

/// Media type by file extension, falling back to plain text for UTF-8 content (when given).
pub fn media_type_of(name: &str, data: Option<&[u8]>) -> &'static str {
    if let Some((_, ext)) = name.rsplit_once('.')
        && !ext.is_empty()
        && ext.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        match ext.to_ascii_lowercase().as_str() {
            "md" | "markdown" => return "text/markdown",
            "txt" | "ts" | "tsx" => return "text/plain",
            "csv" => return "text/csv",
            "html" => return "text/html",
            "css" => return "text/css",
            "js" | "mjs" => return "text/javascript",
            "json" => return "application/json",
            "xml" => return "application/xml",
            "svg" => return "image/svg+xml",
            "png" => return "image/png",
            "jpg" | "jpeg" => return "image/jpeg",
            "gif" => return "image/gif",
            "webp" => return "image/webp",
            "pdf" => return "application/pdf",
            _ => {}
        }
    }
    if data.is_some_and(|bytes| std::str::from_utf8(bytes).is_ok()) {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

async fn directory(value: &Path) -> Result<()> {
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(value)
        .await
        .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
    let stat = tokio::fs::symlink_metadata(value)
        .await
        .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
    if !stat.is_dir() || stat.file_type().is_symlink() {
        return Err(AppError::new("unsafeWorkspace", 409));
    }
    Ok(())
}

fn safe_path(relative: &str) -> Result<&str> {
    if relative.is_empty() || utf16_len(relative) > 2048 {
        return Err(AppError::new("unsafeWorkspace", 409));
    }
    for part in relative.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part == ".git"
            || part == ".codex"
            || part.chars().any(|c| (c as u32) < 0x20 || c == '\\')
        {
            return Err(AppError::new("unsafeWorkspace", 409));
        }
    }
    Ok(relative)
}

async fn check_workspace_growth(workspace: &Path) -> Result<()> {
    let mut stack = vec![workspace.to_path_buf()];
    let mut size: u64 = 0;
    let mut count: usize = 0;
    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .map_err(|_| AppError::new("workspaceTooLarge", 413))?;
        while let Some(item) = entries
            .next_entry()
            .await
            .map_err(|_| AppError::new("workspaceTooLarge", 413))?
        {
            count += 1;
            if count > 10_000 {
                return Err(AppError::new("workspaceTooLarge", 413));
            }
            let path = item.path();
            let stat = tokio::fs::symlink_metadata(&path)
                .await
                .map_err(|_| AppError::new("workspaceTooLarge", 413))?;
            if stat.file_type().is_symlink() {
                continue;
            }
            if stat.is_dir() {
                stack.push(path);
            } else {
                let len = stat.len();
                size = size.saturating_add(len);
                if len > 16 * 1024 * 1024 || size > 64 * 1024 * 1024 {
                    return Err(AppError::new("workspaceTooLarge", 413));
                }
            }
        }
    }
    Ok(())
}

pub async fn git(workspace: &Path, args: &[&str], cancel: CancelToken) -> Result<String> {
    cancel.throw_if_cancelled()?;
    let mut cmd = Command::new("git");
    cmd.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "protocol.file.allow=never",
        "-c",
        "http.followRedirects=false",
    ])
    .args(args)
    .current_dir(workspace)
    .process_group(0)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .env_clear()
    .env("HOME", workspace)
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_TERMINAL_PROMPT", "0");
    if let Ok(path_env) = std::env::var("PATH") {
        cmd.env("PATH", path_env);
    }

    let mut child = cmd.spawn().map_err(|_| AppError::new("gitUnavailable", 503))?;
    let pid = child.id().unwrap_or(0);
    let mut stdout = child.stdout.take().expect("git stdout");
    let mut stderr = child.stderr.take().expect("git stderr");

    let reason: Arc<Mutex<Option<AppError>>> = Arc::new(Mutex::new(None));
    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let total_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let stop = |reason: &Arc<Mutex<Option<AppError>>>, err: AppError| {
        let reason = reason.clone();
        async move {
            let mut guard = reason.lock().await;
            if guard.is_none() {
                *guard = Some(err);
            }
            drop(guard);
            kill_process_group(pid);
        }
    };

    let reason_out = reason.clone();
    let bytes_out = total_bytes.clone();
    let out_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let prev = bytes_out.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                    if prev + n > 1024 * 1024 {
                        let mut guard = reason_out.lock().await;
                        if guard.is_none() {
                            *guard = Some(AppError::new("operationTooLarge", 413));
                        }
                        kill_process_group(pid);
                        break;
                    }
                    collected.extend_from_slice(&buf[..n]);
                }
                Err(_) => break,
            }
        }
        collected
    });

    let reason_err = reason.clone();
    let bytes_err = total_bytes.clone();
    let err_task = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let prev = bytes_err.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                    if prev + n > 1024 * 1024 {
                        let mut guard = reason_err.lock().await;
                        if guard.is_none() {
                            *guard = Some(AppError::new("operationTooLarge", 413));
                        }
                        kill_process_group(pid);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Bound provisioning growth as well as the final materialized source.
    let guard_workspace = workspace.to_path_buf();
    let guard_stopped = stopped.clone();
    let guard_reason = reason.clone();
    let guard_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        interval.tick().await;
        loop {
            interval.tick().await;
            if guard_stopped.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            if let Err(err) = check_workspace_growth(&guard_workspace).await {
                if !guard_stopped.load(std::sync::atomic::Ordering::Relaxed) {
                    let mut guard = guard_reason.lock().await;
                    if guard.is_none() {
                        *guard = Some(err);
                    }
                    drop(guard);
                    kill_process_group(pid);
                }
                break;
            }
        }
    });

    let status = tokio::select! {
        res = child.wait() => res.ok(),
        _ = tokio::time::sleep(Duration::from_secs(60)) => {
            stop(&reason, AppError::new("gitTimedOut", 504)).await;
            child.wait().await.ok()
        }
        _ = cancel.cancelled() => {
            stop(&reason, AppError::new("runStopped", 409)).await;
            child.wait().await.ok()
        }
    };

    stopped.store(true, std::sync::atomic::Ordering::Relaxed);
    guard_task.abort();
    let _ = guard_task.await;
    let out_bytes = out_task.await.unwrap_or_default();
    let _ = err_task.await;

    if let Some(err) = reason.lock().await.clone() {
        return Err(err);
    }
    match status {
        Some(s) if s.code() == Some(0) => Ok(String::from_utf8_lossy(&out_bytes).into_owned()),
        _ => Err(AppError::new("gitFailed", 502)),
    }
}

/// Reject links/special files and bound scans before any file is read into a result.
pub async fn scan(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut result = BTreeMap::new();
    let mut bytes: u64 = 0;
    let mut visited: usize = 0;
    let mut stack: Vec<(String, usize)> = vec![(String::new(), 0)];

    while let Some((relative, depth)) = stack.pop() {
        if depth > 32 {
            return Err(AppError::new("workspaceTooLarge", 413));
        }
        let dir_path = if relative.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&relative)
        };
        let mut entries = tokio::fs::read_dir(&dir_path)
            .await
            .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
        let mut rows = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| AppError::new("unsafeWorkspace", 409))?
        {
            rows.push(entry);
        }
        for row in rows {
            visited += 1;
            if visited > 500 {
                return Err(AppError::new("workspaceTooLarge", 413));
            }
            let entry_name = row
                .file_name()
                .into_string()
                .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
            if entry_name == ".git" || entry_name == ".codex" {
                continue;
            }
            let full_rel = format!("{relative}{entry_name}");
            let name = safe_path(&full_rel)?.to_owned();
            let filename = root.join(&name);
            let stat = tokio::fs::symlink_metadata(&filename)
                .await
                .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
            if stat.file_type().is_symlink() || (!stat.is_file() && !stat.is_dir()) {
                return Err(AppError::with(
                    "unsafeWorkspace",
                    409,
                    "Projects cannot contain symbolic links or special files.",
                ));
            }
            if stat.is_dir() {
                stack.push((format!("{name}/"), depth + 1));
                continue;
            }
            let size = stat.len();
            bytes = bytes.saturating_add(size);
            if result.len() >= 200 || size > 1024 * 1024 || bytes > 8 * 1024 * 1024 {
                return Err(AppError::with(
                    "workspaceTooLarge",
                    413,
                    "This workspace exceeds 200 files, 1 MiB per file or 8 MiB total.",
                ));
            }
            let mut file = tokio::fs::OpenOptions::new()
                .read(true)
                .custom_flags(O_NOFOLLOW)
                .open(&filename)
                .await
                .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
            let meta = file
                .metadata()
                .await
                .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
            if !meta.is_file() {
                return Err(AppError::new("unsafeWorkspace", 409));
            }
            let expected = size as usize;
            let mut buffer = vec![0u8; expected + 1];
            let mut offset = 0;
            while offset < buffer.len() {
                let n = file
                    .read(&mut buffer[offset..])
                    .await
                    .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
                if n == 0 {
                    break;
                }
                offset += n;
            }
            let after_meta = file
                .metadata()
                .await
                .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
            if offset != expected || after_meta.len() != size {
                return Err(AppError::with(
                    "workspaceChanged",
                    409,
                    "A workspace file changed while it was being inspected.",
                ));
            }
            buffer.truncate(offset);
            result.insert(name, buffer);
        }
    }
    Ok(result)
}

pub struct PreparedWorkspace {
    pub workspace: PathBuf,
    source: Source,
    baseline: Baseline,
}

impl PreparedWorkspace {
    pub async fn validate(&self) -> Result<()> {
        let _ = scan(&self.workspace).await?;
        Ok(())
    }

    pub async fn changes(&self) -> Result<(Vec<Value>, bool)> {
        let after = scan(&self.workspace).await?;
        let mut before = BTreeMap::new();
        for row in &self.baseline.files {
            if let Ok(bytes) = B64.decode(&row.content) {
                before.insert(row.path.clone(), bytes);
            }
        }
        let mut names = BTreeSet::new();
        for k in before.keys() {
            names.insert(k.clone());
        }
        for k in after.keys() {
            names.insert(k.clone());
        }

        let mut changes = Vec::new();
        let mut remaining: usize = 180_000;
        let mut truncated = false;

        for name in names {
            let old = before.get(&name);
            let next = after.get(&name);
            if let (Some(a), Some(b)) = (old, next)
                && a == b
            {
                continue;
            }
            let kind = match (old, next) {
                (Some(_), Some(_)) => "modified",
                (Some(_), None) => "deleted",
                (None, Some(_)) => "added",
                (None, None) => continue,
            };
            let is_text = old.is_none_or(|b| std::str::from_utf8(b).is_ok())
                && next.is_none_or(|b| std::str::from_utf8(b).is_ok());
            let full = if is_text {
                let removed = old
                    .map(|b| {
                        std::str::from_utf8(b)
                            .unwrap_or_default()
                            .split('\n')
                            .map(|line| format!("-{line}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                let added = next
                    .map(|b| {
                        std::str::from_utf8(b)
                            .unwrap_or_default()
                            .split('\n')
                            .map(|line| format!("+{line}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                format!("--- a/{name}\n+++ b/{name}\n@@ {kind} @@\n{removed}\n{added}")
            } else {
                "Binary content changed.".to_owned()
            };
            let limit = remaining.min(32_768);
            let diff = slice_utf16(&full, limit).to_owned();
            let diff_units = utf16_len(&diff);
            remaining = remaining.saturating_sub(diff_units);
            if diff.len() < full.len() {
                truncated = true;
            }
            let file = self.source.files.iter().find(|row| row.path == name);
            let content = if kind == "modified"
                && is_text
                && let Some(bytes) = next
                && bytes.len() <= 65_536
                && file.is_some()
            {
                std::str::from_utf8(bytes).ok().map(str::to_owned)
            } else {
                None
            };

            let mut obj = Map::new();
            obj.insert("path".into(), json!(name));
            obj.insert("kind".into(), json!(kind));
            obj.insert("diff".into(), json!(diff));
            if let (Some(text), Some(f)) = (content, file) {
                let text_units = utf16_len(&text);
                if remaining >= text_units {
                    remaining -= text_units;
                    obj.insert("content".into(), json!(text));
                    obj.insert("fileId".into(), json!(f.id));
                    obj.insert("expectedRevision".into(), json!(f.revision));
                    obj.insert("mediaType".into(), json!(f.media_type));
                }
            }
            changes.push(Value::Object(obj));
        }

        Ok((changes, truncated))
    }
}

async fn read_baseline_nofollow(path: &Path) -> std::io::Result<Baseline> {
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
        .await?;
    let mut text = String::new();
    file.read_to_string(&mut text).await?;
    serde_json::from_str(&text).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_workspace(
    root: &Path,
    user_id: &str,
    thread_id: &str,
    source: Source,
    config: &CodexConfig,
    db: &Database,
    session_id: &str,
    cancel: CancelToken,
) -> Result<PreparedWorkspace> {
    let base = root.join("agent-workspaces");
    let owner = base.join(user_id);
    let thread = owner.join(thread_id);
    let workspace = thread.join("work");
    let baseline_path = thread.join("baseline.json");

    for dir in [&base, &owner, &thread] {
        directory(dir).await?;
    }

    let baseline = match read_baseline_nofollow(&baseline_path).await {
        Ok(existing) => {
            directory(&workspace).await?;
            if existing.source.project.source_type != source.project.source_type
                || existing.source.project.source_ref != source.project.source_ref
                || (source.project.source_type == "files" && existing.source.files != source.files)
            {
                return Err(AppError::with(
                    "sourceChanged",
                    409,
                    "The source changed after this thread\u{2019}s workspace was created. Start a new thread to use the current source.",
                ));
            }
            let _ = scan(&workspace).await?;
            existing
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // Never overwrite a partial/unknown working copy left by a failed provisioning attempt.
            match tokio::fs::symlink_metadata(&workspace).await {
                Ok(_) => {
                    return Err(AppError::with(
                        "workspaceIncomplete",
                        409,
                        "Workspace setup was interrupted. Start a new thread; existing files were retained.",
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(AppError::internal()),
            }
            if source.project.source_type == "git" {
                let url_str = &source.project.source_ref;
                if !url_str.starts_with("https://") {
                    return Err(AppError::with(
                        "gitAuthenticationUnavailable",
                        409,
                        "Use a credential-free HTTPS repository. SSH credential provisioning is not connected yet.",
                    ));
                }
                let host = url::Url::parse(url_str)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_lowercase))
                    .unwrap_or_default();
                if !config.git_hosts.iter().any(|h| h == &host) {
                    return Err(AppError::with(
                        "gitHostDenied",
                        403,
                        "This repository host is not enabled by the administrator.",
                    ));
                }
                if git(
                    &thread,
                    &[
                        "clone",
                        "--depth=1",
                        "--single-branch",
                        "--no-hardlinks",
                        "--",
                        url_str,
                        "work",
                    ],
                    cancel.clone(),
                )
                .await
                .is_err()
                {
                    return Err(AppError::with(
                        "cloneFailed",
                        502,
                        "The repository could not be cloned. Verify the URL, allowed host and public access.",
                    ));
                }
                let tree = git(&workspace, &["ls-tree", "-r", "HEAD"], cancel.clone()).await?;
                if tree.split('\n').any(|line| line.starts_with("160000 ")) {
                    return Err(AppError::with(
                        "submoduleUnavailable",
                        409,
                        "Submodules are not materialized yet. Choose a repository without submodules.",
                    ));
                }
                // Repository config is not execution authority. Only the isolated copy is filtered.
                let _ = tokio::fs::remove_dir_all(workspace.join(".codex")).await;
            } else {
                directory(&workspace).await?;
                for row in &source.files {
                    cancel.throw_if_cancelled()?;
                    let entry: BlobFile = serde_json::from_value(
                        db.read("service_download", json!([session_id, row.id, ""]))
                            .await?,
                    )
                    .map_err(|_| AppError::new("contentUnavailable", 503))?;
                    if entry.revision != row.revision || (entry.size as u64) != row.size {
                        return Err(AppError::with(
                            "conflict",
                            409,
                            "A source file changed during workspace setup. Start a new thread.",
                        ));
                    }
                    let destination = workspace.join(safe_path(&row.path)?);
                    if let Some(parent) = destination.parent() {
                        directory(parent).await?;
                    }
                    if !is_blob_id(&entry.blob_id) {
                        return Err(AppError::new("invalidBlobId", 400));
                    }
                    let blob = root.join("blobs").join(&entry.blob_id);
                    let mut input = tokio::fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(O_NOFOLLOW)
                        .open(&blob)
                        .await
                        .map_err(|_| AppError::new("contentUnavailable", 503))?;
                    let stat = input
                        .metadata()
                        .await
                        .map_err(|_| AppError::new("contentUnavailable", 503))?;
                    if !stat.is_file() || stat.len() != row.size {
                        return Err(AppError::new("contentUnavailable", 503));
                    }
                    let mut data = Vec::with_capacity(row.size as usize);
                    input
                        .read_to_end(&mut data)
                        .await
                        .map_err(|_| AppError::new("contentUnavailable", 503))?;
                    let mut out = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&destination)
                        .await
                        .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
                    out.write_all(&data)
                        .await
                        .map_err(|_| AppError::internal())?;
                }
                git(&workspace, &["init", "-b", "agent"], cancel.clone()).await?;
                git(&workspace, &["add", "--all"], cancel.clone()).await?;
                git(
                    &workspace,
                    &[
                        "-c",
                        "user.name=Dar",
                        "-c",
                        "user.email=agent@dar.invalid",
                        "commit",
                        "--allow-empty",
                        "-m",
                        "Dar workspace baseline",
                    ],
                    cancel.clone(),
                )
                .await?;
            }
            let scanned = scan(&workspace).await?;
            let baseline = Baseline {
                source: source.clone(),
                files: scanned
                    .into_iter()
                    .map(|(path, data)| BaselineFile {
                        path,
                        content: B64.encode(data),
                    })
                    .collect(),
            };
            let serialized = serde_json::to_vec(&baseline).map_err(|_| AppError::internal())?;
            let bp = baseline_path.clone();
            tokio::task::spawn_blocking(move || create_durable_file(&bp, &serialized))
                .await
                .map_err(|_| AppError::internal())?
                .map_err(|_| AppError::internal())?;
            baseline
        }
        Err(_) => return Err(AppError::new("unsafeWorkspace", 409)),
    };

    // Cached workspaces from older Dar versions may predate this deny root.
    // Both 0.163 and 0.166 need it to exist before bubblewrap masks the directory.
    directory(&workspace.join(".codex")).await?;

    Ok(PreparedWorkspace {
        workspace,
        source,
        baseline,
    })
}

pub struct BranchChange {
    pub path: String,
    pub kind: &'static str,
    pub data: Option<Vec<u8>>,
    pub media_type: Option<String>,
    pub file_id: Option<String>,
    pub expect_revision: Option<u32>,
}

/// Everything a Files thread changed against its baseline, with full contents, for its history branch.
pub async fn branch_changes(root: &Path, user_id: &str, thread_id: &str) -> Result<Vec<BranchChange>> {
    let thread = root.join("agent-workspaces").join(user_id).join(thread_id);
    let baseline = match read_baseline_nofollow(&thread.join("baseline.json")).await {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::with(
                "workspaceUnavailable",
                409,
                "Run the agent in this thread before committing its changes.",
            ));
        }
        Err(_) => return Err(AppError::new("unsafeWorkspace", 409)),
    };
    if baseline.source.project.source_type != "files" {
        return Err(AppError::invalid());
    }
    let after = scan(&thread.join("work")).await?;
    let mut before = BTreeMap::new();
    for row in &baseline.files {
        if let Ok(bytes) = B64.decode(&row.content) {
            before.insert(row.path.clone(), bytes);
        }
    }
    let mut names = BTreeSet::new();
    for k in before.keys() {
        names.insert(k.clone());
    }
    for k in after.keys() {
        names.insert(k.clone());
    }

    let mut result = Vec::new();
    for name in names {
        let old = before.get(&name);
        let next = after.get(&name);
        if let (Some(a), Some(b)) = (old, next)
            && a == b
        {
            continue;
        }
        let file = baseline.source.files.iter().find(|row| row.path == name);
        if old.is_some() && file.is_none() {
            continue;
        }
        let kind = match (old, next) {
            (Some(_), Some(_)) => "modified",
            (Some(_), None) => "deleted",
            (None, Some(_)) => "added",
            (None, None) => continue,
        };
        let (data, media_type) = match next {
            Some(bytes) => {
                let mt = file
                    .map(|f| f.media_type.clone())
                    .unwrap_or_else(|| media_type_of(&name, Some(bytes)).to_owned());
                (Some(bytes.clone()), Some(mt))
            }
            None => (None, None),
        };
        result.push(BranchChange {
            path: name,
            kind,
            data,
            media_type,
            file_id: file.map(|f| f.id.clone()),
            expect_revision: file.map(|f| f.revision),
        });
    }
    Ok(result)
}
