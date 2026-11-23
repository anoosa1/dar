//! Claude Code CLI stream-json client, account check, models and usage limits
//! (`next/runtime/agent/claude.ts`).

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};

use crate::agent::codex::{CODEX_INSTRUCTIONS, CodexConfig, object};
use crate::agent::discovery::{codex_search_directories, find_codex_executable};
use crate::agent::{CancelToken, home_dir, kill_process_group, slice_utf16, valid_effort};
use crate::auth::uuid;
use crate::error::{AppError, Result};

pub type ClaudeConfig = CodexConfig;
pub type Json = Map<String, Value>;
pub type ControlRequestFuture = Pin<Box<dyn Future<Output = Result<Json>> + Send>>;

pub trait ClaudeHandlers: Send + Sync + 'static {
    fn message(&self, value: Json);
    fn request(&self, subtype: String, request: Json) -> ControlRequestFuture;
}

pub struct ClaudeAccount {
    pub plan: String,
}

fn safe_model(s: &str) -> bool {
    (1..=100).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'[' | b']' | b'-'))
}

fn safe_plan(s: &str) -> bool {
    (1..=40).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b' ' | b'-'))
}

fn apply_env(cmd: &mut Command, config: &ClaudeConfig) {
    let path_env = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
    cmd.env_clear()
        .env("PATH", path_env)
        .env("HOME", &config.home)
        .env("CLAUDE_CONFIG_DIR", &config.home)
        .env("LANG", "C.UTF-8")
        .env("NO_COLOR", "1")
        .env("DISABLE_AUTOUPDATER", "1");
}

/// `claude auth status` prints JSON. Only first-party Claude logins are accepted; the email is never returned.
pub async fn claude_account(
    config: &ClaudeConfig,
    workspace: &Path,
    cancel: Option<CancelToken>,
) -> Result<Option<ClaudeAccount>> {
    if let Some(ref c) = cancel {
        c.throw_if_cancelled()?;
    }
    let mut cmd = Command::new(&config.executable);
    cmd.args(["auth", "status"])
        .current_dir(workspace)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    apply_env(&mut cmd, config);

    let mut child = cmd.spawn().map_err(|_| {
        AppError::with(
            "claudeUnavailable",
            503,
            "The Claude Code CLI could not be started.",
        )
    })?;
    let pid = child.id().unwrap_or(0);
    let mut stdout = child.stdout.take().expect("claude stdout");

    let out_task = tokio::spawn(async move {
        let mut collected = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if collected.len() + n > 65_536 {
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

    let status = if let Some(ref c) = cancel {
        tokio::select! {
            res = child.wait() => res.ok(),
            _ = tokio::time::sleep(Duration::from_secs(15)) => {
                kill_process_group(pid);
                child.wait().await.ok()
            }
            _ = c.cancelled() => {
                kill_process_group(pid);
                child.wait().await.ok()
            }
        }
    } else {
        tokio::select! {
            res = child.wait() => res.ok(),
            _ = tokio::time::sleep(Duration::from_secs(15)) => {
                kill_process_group(pid);
                child.wait().await.ok()
            }
        }
    };

    let bytes = out_task.await.unwrap_or_default();
    if let Some(ref c) = cancel {
        c.throw_if_cancelled()?;
    }
    let Ok(parsed) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let Ok(value) = object(parsed) else {
        return Ok(None);
    };
    let code = status.and_then(|s| s.code());
    if code != Some(0)
        || value.get("loggedIn") != Some(&Value::Bool(true))
        || value.get("apiProvider").and_then(Value::as_str) != Some("firstParty")
    {
        return Ok(None);
    }
    let plan = value
        .get("subscriptionType")
        .and_then(Value::as_str)
        .filter(|s| safe_plan(s))
        .unwrap_or("")
        .to_owned();
    Ok(Some(ClaudeAccount { plan }))
}

/// Commands run only inside Claude Code's own sandbox, which needs bubblewrap and socat on Linux.
pub async fn claude_commands_available() -> bool {
    let path_env = std::env::var("PATH").unwrap_or_default();
    let directories = codex_search_directories(&path_env, &home_dir());
    find_codex_executable(&directories, "bwrap").await.is_some()
        && find_codex_executable(&directories, "socat").await.is_some()
}

pub struct ClaudeRunOptions<'a> {
    pub workspace: &'a Path,
    pub model: &'a str,
    pub effort: Option<&'a str>,
    pub plan: bool,
    pub resume: Option<&'a str>,
    pub commands: bool,
    pub deny: &'a [String],
}

pub fn claude_args(opts: &ClaudeRunOptions<'_>) -> Result<Vec<String>> {
    if !safe_model(opts.model) {
        return Err(AppError::new("invalidModel", 400));
    }
    let mut tools = vec![
        "Read",
        "Edit",
        "Write",
        "Glob",
        "Grep",
        "TodoWrite",
        "AskUserQuestion",
    ];
    if opts.commands {
        tools.push("Bash");
    }
    let workspace_str = opts.workspace.to_string_lossy().into_owned();
    let git_dir = opts.workspace.join(".git").to_string_lossy().into_owned();
    let settings = if opts.commands {
        json!({
            "sandbox": {
                "enabled": true,
                "failIfUnavailable": true,
                "autoAllowBashIfSandboxed": true,
                "allowUnsandboxedCommands": false,
                "network": { "allowedDomains": [] },
                "filesystem": {
                    "denyRead": opts.deny,
                    "allowRead": [workspace_str],
                    "allowWrite": [],
                    "denyWrite": [git_dir],
                }
            }
        })
    } else {
        json!({})
    };
    let instructions = if opts.commands {
        CODEX_INSTRUCTIONS.to_owned()
    } else {
        format!("{CODEX_INSTRUCTIONS} Shell commands are unavailable; use the file tools.")
    };
    let mut args = vec![
        "--model".into(),
        opts.model.to_owned(),
        "--tools".into(),
        tools.join(","),
        "--permission-mode".into(),
        if opts.plan { "plan" } else { "acceptEdits" }.into(),
        "--include-partial-messages".into(),
        "--append-system-prompt".into(),
        instructions,
        "--settings".into(),
        settings.to_string(),
    ];
    if let Some(effort) = opts.effort {
        args.push("--effort".into());
        args.push(effort.to_owned());
    }
    if let Some(resume) = opts.resume {
        args.push("--resume".into());
        args.push(resume.to_owned());
    }
    Ok(args)
}

struct Inner {
    pid: u32,
    failure: Option<AppError>,
    ended: bool,
    pending: HashMap<String, oneshot::Sender<Result<Json>>>,
    writer: Option<mpsc::UnboundedSender<String>>,
    stderr: String,
}

#[derive(Clone)]
pub struct ClaudeProcess {
    inner: Arc<Mutex<Inner>>,
    exited_rx: watch::Receiver<Option<Option<i32>>>,
}

impl ClaudeProcess {
    pub fn send(&self, value: &Value) {
        let line = match serde_json::to_string(value) {
            Ok(s) => s,
            Err(_) => return,
        };
        let guard = self.inner.lock().expect("claude lock");
        if guard.failure.is_none()
            && !guard.ended
            && let Some(ref tx) = guard.writer
        {
            let _ = tx.send(line);
        }
    }

    pub async fn control(&self, subtype: &str, mut extra: Json) -> Result<Json> {
        let rx = {
            let mut guard = self.inner.lock().expect("claude lock");
            if let Some(ref err) = guard.failure {
                return Err(err.clone());
            }
            if guard.ended {
                return Err(AppError::new("runStopped", 409));
            }
            let id = uuid();
            let (tx, rx) = oneshot::channel();
            guard.pending.insert(id.clone(), tx);
            extra.insert("subtype".into(), json!(subtype));
            if let Some(ref writer) = guard.writer {
                let line = serde_json::to_string(&json!({
                    "type": "control_request",
                    "request_id": id,
                    "request": Value::Object(extra),
                }))
                .unwrap_or_default();
                let _ = writer.send(line);
            }
            rx
        };
        rx.await
            .unwrap_or_else(|_| Err(AppError::new("runStopped", 409)))
    }

    pub fn end(&self) {
        let mut guard = self.inner.lock().expect("claude lock");
        if !guard.ended {
            guard.ended = true;
            guard.writer = None;
        }
    }

    pub fn close(&self, error: Option<AppError>) {
        let err = error.unwrap_or_else(|| AppError::new("runStopped", 409));
        let (pid, pending, final_err) = {
            let mut guard = self.inner.lock().expect("claude lock");
            if guard.failure.is_none() {
                guard.failure = Some(err);
            }
            let final_err = guard.failure.clone().expect("failure");
            guard.ended = true;
            guard.writer = None;
            let drained: Vec<_> = guard.pending.drain().map(|(_, tx)| tx).collect();
            (guard.pid, drained, final_err)
        };
        for tx in pending {
            let _ = tx.send(Err(final_err.clone()));
        }
        if pid != 0 {
            kill_process_group(pid);
        }
    }

    pub async fn exited(&self) -> Option<i32> {
        let mut rx = self.exited_rx.clone();
        if let Some(code) = *rx.borrow_and_update() {
            return code;
        }
        let _ = rx.wait_for(|v| v.is_some()).await;
        rx.borrow().flatten()
    }

    pub fn diagnostics(&self) -> String {
        self.inner.lock().expect("claude lock").stderr.clone()
    }
}

pub fn open_claude<H: ClaudeHandlers>(
    config: &ClaudeConfig,
    workspace: &Path,
    args: &[String],
    handlers: H,
) -> ClaudeProcess {
    let mut full_args: Vec<String> = vec![
        "-p".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--permission-prompt-tool".into(),
        "stdio".into(),
        "--restricted".into(),
        "--strict-mcp-config".into(),
        "--disable-slash-commands".into(),
    ];
    full_args.extend(args.iter().cloned());

    let mut cmd = Command::new(&config.executable);
    cmd.args(&full_args)
        .current_dir(workspace)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_env(&mut cmd, config);

    let (exited_tx, exited_rx) = watch::channel(None);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            let _ = exited_tx.send(Some(None));
            return ClaudeProcess {
                inner: Arc::new(Mutex::new(Inner {
                    pid: 0,
                    failure: Some(AppError::with(
                        "claudeUnavailable",
                        503,
                        "The Claude Code CLI could not be started.",
                    )),
                    ended: true,
                    pending: HashMap::new(),
                    writer: None,
                    stderr: String::new(),
                })),
                exited_rx,
            };
        }
    };

    let pid = child.id().unwrap_or(0);
    let mut stdin = child.stdin.take().expect("claude stdin");
    let mut stdout = child.stdout.take().expect("claude stdout");
    let mut stderr = child.stderr.take().expect("claude stderr");

    let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<String>();
    let proc = ClaudeProcess {
        inner: Arc::new(Mutex::new(Inner {
            pid,
            failure: None,
            ended: false,
            pending: HashMap::new(),
            writer: Some(writer_tx),
            stderr: String::new(),
        })),
        exited_rx,
    };

    let proc_write = proc.clone();
    tokio::spawn(async move {
        while let Some(mut line) = writer_rx.recv().await {
            line.push('\n');
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                proc_write.close(Some(AppError::new("claudeUnavailable", 503)));
                break;
            }
        }
        let _ = stdin.shutdown().await;
    });

    let proc_err = proc.clone();
    let stderr_task = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]);
                    let mut guard = proc_err.inner.lock().expect("claude lock");
                    if guard.stderr.len() < 16_384 {
                        let remain = 16_384 - guard.stderr.len();
                        let part = slice_utf16(&text, remain);
                        guard.stderr.push_str(part);
                    }
                }
                Err(_) => break,
            }
        }
    });

    let handlers = Arc::new(handlers);
    let proc_read = proc.clone();
    let stdout_task = tokio::spawn(async move {
        const MAX_LINE: usize = 16 * 1024 * 1024;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];

        let handle_line = |raw: &[u8]| -> bool {
            let Ok(text) = std::str::from_utf8(raw) else {
                proc_read.close(Some(AppError::with(
                    "invalidProviderResponse",
                    502,
                    "Claude Code emitted invalid JSON output.",
                )));
                return false;
            };
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return true;
            }
            let Ok(Value::Object(message)) = serde_json::from_str::<Value>(trimmed) else {
                proc_read.close(Some(AppError::with(
                    "invalidProviderResponse",
                    502,
                    "Claude Code emitted invalid JSON output.",
                )));
                return false;
            };

            let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "control_response" {
                let response = message
                    .get("response")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let id = response
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let waiting = proc_read
                    .inner
                    .lock()
                    .expect("claude lock")
                    .pending
                    .remove(&id);
                if let Some(tx) = waiting {
                    if response.get("subtype").and_then(Value::as_str) == Some("success") {
                        let inner = response
                            .get("response")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        let _ = tx.send(Ok(inner));
                    } else {
                        let detail = response
                            .get("error")
                            .and_then(Value::as_str)
                            .map(|s| slice_utf16(s, 500).to_owned())
                            .unwrap_or_else(|| "Claude Code rejected the request.".to_owned());
                        let _ = tx.send(Err(AppError::with("claudeRequestFailed", 502, &detail)));
                    }
                }
            } else if kind == "control_request" {
                let Some(id) = message.get("request_id").and_then(Value::as_str).map(str::to_owned) else {
                    return true;
                };
                let request = message
                    .get("request")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let subtype = request
                    .get("subtype")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let h = handlers.clone();
                let p = proc_read.clone();
                tokio::spawn(async move {
                    match h.request(subtype, request).await {
                        Ok(response) => p.send(&json!({
                            "type": "control_response",
                            "response": {
                                "subtype": "success",
                                "request_id": id,
                                "response": Value::Object(response),
                            }
                        })),
                        Err(err) => p.send(&json!({
                            "type": "control_response",
                            "response": {
                                "subtype": "error",
                                "request_id": id,
                                "error": err.message,
                            }
                        })),
                    }
                });
            } else if kind != "control_cancel_request" && kind != "keep_alive" {
                handlers.message(message);
            }
            true
        };

        loop {
            match stdout.read(&mut chunk).await {
                Ok(0) => {
                    if !buf.is_empty() {
                        let _ = handle_line(&buf);
                    }
                    break;
                }
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line = buf.drain(..=pos).collect::<Vec<u8>>();
                        if line.len() - 1 > MAX_LINE {
                            proc_read.close(Some(AppError::with(
                                "invalidProviderResponse",
                                502,
                                "Claude Code emitted invalid JSON output.",
                            )));
                            return;
                        }
                        if !handle_line(&line[..line.len() - 1]) {
                            return;
                        }
                    }
                    if buf.len() > MAX_LINE {
                        proc_read.close(Some(AppError::with(
                            "invalidProviderResponse",
                            502,
                            "Claude Code emitted invalid JSON output.",
                        )));
                        return;
                    }
                }
                Err(_) => {
                    proc_read.close(Some(AppError::new("claudeUnavailable", 503)));
                    break;
                }
            }
        }
    });

    let proc_exit = proc.clone();
    tokio::spawn(async move {
        let status = child.wait().await.ok();
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        proc_exit.close(Some(AppError::with(
            "claudeIncomplete",
            502,
            "Claude Code exited before completing the turn.",
        )));
        let _ = exited_tx.send(Some(status.and_then(|s| s.code())));
    });

    proc
}

struct InspectHandlers;
impl ClaudeHandlers for InspectHandlers {
    fn message(&self, _value: Json) {}
    fn request(&self, _subtype: String, _request: Json) -> ControlRequestFuture {
        Box::pin(async { Err(AppError::new("declined", 403)) })
    }
}

async fn inspect<T, F, Fut>(
    config: &ClaudeConfig,
    workspace: &Path,
    cancel: Option<CancelToken>,
    read: F,
) -> Result<T>
where
    F: FnOnce(ClaudeProcess, Json) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let args = vec!["--tools".to_owned(), String::new()];
    let claude = open_claude(config, workspace, &args, InspectHandlers);

    let claude_timeout = claude.clone();
    let timer = tokio::spawn(async move {
        if let Some(c) = cancel {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(20)) => {}
                _ = c.cancelled() => {}
            }
        } else {
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
        claude_timeout.close(Some(AppError::with(
            "claudeTimedOut",
            504,
            "The Claude Code CLI timed out.",
        )));
    });

    let result = async {
        let init = claude.control("initialize", Map::new()).await?;
        read(claude.clone(), init).await
    }
    .await;

    timer.abort();
    claude.end();
    claude.close(None);
    result
}

pub async fn claude_models(
    config: &ClaudeConfig,
    workspace: &Path,
    cancel: Option<CancelToken>,
) -> Result<Vec<Value>> {
    inspect(config, workspace, cancel, |_claude, init| async move {
        let raw_models = init
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::new("invalidProviderResponse", 502))?;
        let mut rows: Vec<Json> = Vec::new();
        for item in raw_models.iter().take(100) {
            rows.push(object(item.clone())?);
        }
        let fallback = rows
            .iter()
            .find(|r| r.get("value").and_then(Value::as_str) == Some("default"))
            .and_then(|r| r.get("resolvedModel").and_then(Value::as_str))
            .map(str::to_owned);

        let mut marked = false;
        let mut out = Vec::new();
        for row in rows {
            let Some(val) = row.get("value").and_then(Value::as_str) else {
                continue;
            };
            if val == "default" || !safe_model(val) {
                continue;
            }
            let Some(display_name) = row.get("displayName").and_then(Value::as_str) else {
                continue;
            };
            let efforts: Vec<String> = row
                .get("supportedEffortLevels")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .filter(|s| valid_effort(s))
                        .take(10)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let resolved = row.get("resolvedModel").and_then(Value::as_str);
            let is_default = !marked && resolved.is_some() && resolved == fallback.as_deref();
            if is_default {
                marked = true;
            }
            let mut item = json!({
                "id": format!("claude/{val}"),
                "name": slice_utf16(display_name, 160),
                "isDefault": is_default,
                "provider": "claude",
            });
            if !efforts.is_empty() {
                item["efforts"] = json!(efforts);
            }
            out.push(item);
            if out.len() >= 50 {
                break;
            }
        }
        Ok(out)
    })
    .await
}

/// Subscription windows from the CLI's own usage report; nothing is cached or estimated.
pub async fn claude_limits(config: &ClaudeConfig, workspace: &Path) -> Result<Value> {
    inspect(config, workspace, None, |claude, _init| async move {
        let usage = claude.control("get_usage", Map::new()).await?;
        let limits = usage
            .get("rate_limits")
            .and_then(Value::as_object)
            .and_then(|o| o.get("limits"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut windows = Vec::new();
        for item in limits.into_iter().take(8) {
            let Some(row) = item.as_object() else {
                continue;
            };
            let Some(percent) = row
                .get("percent")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .map(|v| v.clamp(0.0, 100.0))
            else {
                continue;
            };
            let model = row
                .get("scope")
                .and_then(Value::as_object)
                .and_then(|s| s.get("model"))
                .and_then(Value::as_object)
                .and_then(|m| m.get("display_name"))
                .and_then(Value::as_str)
                .map(|d| format!(" \u{00b7} {}", slice_utf16(d, 40)))
                .unwrap_or_default();
            let session = row.get("group").and_then(Value::as_str) == Some("session");
            let resets_at = row
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis());
            windows.push(json!({
                "label": format!("Claude Code \u{00b7} {}{model}", if session { "5 hour" } else { "Weekly" }),
                "usedPercent": percent,
                "windowMinutes": if session { 300 } else { 10080 },
                "resetsAt": resets_at,
            }));
        }
        let plan = usage
            .get("subscription_type")
            .and_then(Value::as_str)
            .filter(|s| safe_plan(s))
            .map(str::to_owned);
        let available = usage.get("rate_limits_available") != Some(&Value::Bool(false));
        Ok(json!({
            "available": available,
            "plan": plan,
            "windows": windows,
        }))
    })
    .await
}
