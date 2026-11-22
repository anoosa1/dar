//! `codex app-server` stdio JSON-RPC client (`next/runtime/agent/app-server.ts`).

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};

use crate::agent::codex::{CodexConfig, options};
use crate::agent::{kill_process_group, slice_utf16};
use crate::error::{AppError, Result};

pub type Json = Map<String, Value>;
pub type RequestFuture = Pin<Box<dyn Future<Output = Result<Value>> + Send>>;

pub trait AppServerHandlers: Send + Sync + 'static {
    fn notify(&self, method: &str, params: Json);
    fn request(&self, method: String, params: Json) -> RequestFuture;
}

/// Execution-policy rule files cannot be ignored by app-server, so their presence blocks execution.
pub async fn assert_no_codex_rules(config: &CodexConfig) -> Result<()> {
    let rules_dir = Path::new(&config.home).join("rules");
    if let Ok(mut entries) = tokio::fs::read_dir(&rules_dir).await
        && entries.next_entry().await.ok().flatten().is_some()
    {
        return Err(AppError::with(
            "codexRulesPresent",
            409,
            "The Dar Codex home contains execution-policy rules, which Dar cannot ignore. Ask the administrator to remove its rules directory.",
        ));
    }
    Ok(())
}

struct Inner {
    pid: u32,
    next_id: u64,
    failure: Option<AppError>,
    pending: HashMap<u64, oneshot::Sender<Result<Value>>>,
    writer: Option<mpsc::UnboundedSender<String>>,
}

#[derive(Clone)]
pub struct AppServer {
    inner: Arc<Mutex<Inner>>,
    exited_rx: watch::Receiver<Option<Option<i32>>>,
}

impl AppServer {
    fn write_json(&self, value: &Value) {
        let line = match serde_json::to_string(value) {
            Ok(s) => s,
            Err(_) => return,
        };
        let guard = self.inner.lock().expect("app_server lock");
        if guard.failure.is_none()
            && let Some(ref tx) = guard.writer
        {
            let _ = tx.send(line);
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.write_json(&json!({ "method": method, "params": params }));
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let rx = {
            let mut guard = self.inner.lock().expect("app_server lock");
            if let Some(ref err) = guard.failure {
                return Err(err.clone());
            }
            let id = guard.next_id;
            guard.next_id += 1;
            let (tx, rx) = oneshot::channel();
            guard.pending.insert(id, tx);
            if let Some(ref writer) = guard.writer {
                let line = serde_json::to_string(&json!({
                    "id": id,
                    "method": method,
                    "params": params,
                }))
                .unwrap_or_default();
                let _ = writer.send(line);
            }
            rx
        };
        rx.await
            .unwrap_or_else(|_| Err(AppError::new("runStopped", 409)))
    }

    pub fn close(&self, error: Option<AppError>) {
        let err = error.unwrap_or_else(|| AppError::new("runStopped", 409));
        let (pid, pending, final_err) = {
            let mut guard = self.inner.lock().expect("app_server lock");
            if guard.failure.is_none() {
                guard.failure = Some(err);
            }
            let final_err = guard.failure.clone().expect("failure");
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
}

/// One `codex app-server` process per Dar run. Messages are newline-delimited JSON-RPC
/// on stdio. Credentials stay in the server-side Codex home; stderr is never forwarded.
pub fn open_app_server<H: AppServerHandlers>(
    config: &CodexConfig,
    workspace: &Path,
    handlers: H,
    extra: &[&str],
) -> AppServer {
    let mut args = vec!["app-server".to_owned()];
    args.extend(options(config, workspace));
    args.push("-c".into());
    args.push("notify=[]".into());
    args.extend(extra.iter().map(|s| (*s).to_owned()));

    let mut cmd = Command::new(&config.executable);
    cmd.args(&args)
        .current_dir(workspace)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .env("HOME", &config.home)
        .env("CODEX_HOME", &config.home)
        .env("LANG", "C.UTF-8")
        .env("NO_COLOR", "1");
    if let Ok(path_env) = std::env::var("PATH") {
        cmd.env("PATH", path_env);
    }

    let (exited_tx, exited_rx) = watch::channel(None);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            let _ = exited_tx.send(Some(None));
            return AppServer {
                inner: Arc::new(Mutex::new(Inner {
                    pid: 0,
                    next_id: 1,
                    failure: Some(AppError::with(
                        "codexUnavailable",
                        503,
                        "The Codex CLI could not be started.",
                    )),
                    pending: HashMap::new(),
                    writer: None,
                })),
                exited_rx,
            };
        }
    };

    let pid = child.id().unwrap_or(0);
    let mut stdin = child.stdin.take().expect("app_server stdin");
    let mut stdout = child.stdout.take().expect("app_server stdout");
    let mut stderr = child.stderr.take().expect("app_server stderr");

    let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<String>();
    let server = AppServer {
        inner: Arc::new(Mutex::new(Inner {
            pid,
            next_id: 1,
            failure: None,
            pending: HashMap::new(),
            writer: Some(writer_tx),
        })),
        exited_rx,
    };

    let server_write = server.clone();
    tokio::spawn(async move {
        while let Some(mut line) = writer_rx.recv().await {
            line.push('\n');
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                server_write.close(Some(AppError::new("codexUnavailable", 503)));
                break;
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
        }
    });

    let handlers = Arc::new(handlers);
    let server_read = server.clone();
    let stdout_task = tokio::spawn(async move {
        const MAX_LINE: usize = 8 * 1024 * 1024;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        let handle_line = |raw: &[u8]| -> bool {
            let Ok(text) = std::str::from_utf8(raw) else {
                server_read.close(Some(AppError::with(
                    "invalidProviderResponse",
                    502,
                    "Codex emitted invalid JSON output.",
                )));
                return false;
            };
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return true;
            }
            let Ok(Value::Object(message)) = serde_json::from_str::<Value>(trimmed) else {
                server_read.close(Some(AppError::with(
                    "invalidProviderResponse",
                    502,
                    "Codex emitted invalid JSON output.",
                )));
                return false;
            };
            let id = message.get("id").cloned();
            let method = message.get("method").and_then(Value::as_str).map(str::to_owned);
            let params = message
                .get("params")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();

            if let Some(method) = method {
                if let Some(id_val) = id.filter(|v| v.is_number() || v.is_string()) {
                    let h = handlers.clone();
                    let srv = server_read.clone();
                    tokio::spawn(async move {
                        match h.request(method, params).await {
                            Ok(result) => srv.write_json(&json!({ "id": id_val, "result": result })),
                            Err(err) => srv.write_json(&json!({
                                "id": id_val,
                                "error": { "code": -32000, "message": err.message },
                            })),
                        }
                    });
                } else {
                    handlers.notify(&method, params);
                }
            } else if let Some(req_id) = id.and_then(|v| v.as_u64()) {
                let waiting = server_read
                    .inner
                    .lock()
                    .expect("app_server lock")
                    .pending
                    .remove(&req_id);
                if let Some(tx) = waiting {
                    if let Some(err_val) = message.get("error").filter(|v| !v.is_null()) {
                        let detail = err_val
                            .as_object()
                            .and_then(|o| o.get("message"))
                            .and_then(Value::as_str)
                            .map(|s| slice_utf16(s, 500).to_owned())
                            .unwrap_or_else(|| "Codex rejected the request.".to_owned());
                        let _ = tx.send(Err(AppError::with("codexRequestFailed", 502, &detail)));
                    } else {
                        let result = message.get("result").cloned().unwrap_or(Value::Null);
                        let _ = tx.send(Ok(result));
                    }
                }
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
                            server_read.close(Some(AppError::with(
                                "invalidProviderResponse",
                                502,
                                "Codex emitted invalid JSON output.",
                            )));
                            return;
                        }
                        if !handle_line(&line[..line.len() - 1]) {
                            return;
                        }
                    }
                    if buf.len() > MAX_LINE {
                        server_read.close(Some(AppError::with(
                            "invalidProviderResponse",
                            502,
                            "Codex emitted invalid JSON output.",
                        )));
                        return;
                    }
                }
                Err(_) => {
                    server_read.close(Some(AppError::new("codexUnavailable", 503)));
                    break;
                }
            }
        }
    });

    let server_exit = server.clone();
    tokio::spawn(async move {
        let status = child.wait().await.ok();
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        server_exit.close(Some(AppError::with(
            "codexIncomplete",
            502,
            "The Codex CLI exited before reporting a completed turn.",
        )));
        let _ = exited_tx.send(Some(status.and_then(|s| s.code())));
    });

    server
}
