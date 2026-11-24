//! Supervises `dar-chat-bridge`: newline-delimited JSON over stdio (`next/runtime/chat/bridge.ts`).
//! The process restarts with backoff after a crash; `on_ready` runs after every start so
//! accounts reconnect. `close()` stops it and fails anything still waiting.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;

use crate::error::{AppError, Result};

const PROTOCOL_VERSION: i64 = 1;

#[derive(Clone, Debug)]
pub(crate) struct BridgeStatus {
    pub available: bool,
    pub detail: String,
}

/// Callbacks supplied by `ChatRuntime` for incoming bridge requests, events, and readiness.
pub(crate) trait BridgeHandler: Send + Sync + 'static {
    fn request(
        &self,
        method: String,
        params: Map<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>>;
    fn event(&self, account: String, event: String, data: Map<String, Value>);
    fn on_ready(&self);
}

struct BridgeState {
    closing: bool,
    ready: bool,
    restarts: u32,
    generation: u64,
    next_id: u64,
    status: BridgeStatus,
    stdin_tx: Option<mpsc::UnboundedSender<String>>,
    child_pid: Option<u32>,
    kill_tx: Option<oneshot::Sender<()>>,
    exit_rx: Option<watch::Receiver<bool>>,
    restart_task: Option<AbortHandle>,
    pending: HashMap<String, oneshot::Sender<Result<Value>>>,
}

struct BridgeInner {
    executable: Option<String>,
    env: HashMap<String, String>,
    state: Mutex<BridgeState>,
    handler: Mutex<Option<Arc<dyn BridgeHandler>>>,
}

#[derive(Clone)]
pub(crate) struct Bridge {
    inner: Arc<BridgeInner>,
}

impl Bridge {
    /// Creates the bridge supervisor in idle state; call [`Bridge::start`] once handlers are ready.
    pub fn new(executable: Option<String>, env: HashMap<String, String>) -> Self {
        let detail = if executable.is_some() {
            "Starting the chat bridge\u{2026}".to_owned()
        } else {
            "The chat bridge is not installed on this server.".to_owned()
        };
        Self {
            inner: Arc::new(BridgeInner {
                executable,
                env,
                state: Mutex::new(BridgeState {
                    closing: false,
                    ready: false,
                    restarts: 0,
                    generation: 0,
                    next_id: 0,
                    status: BridgeStatus {
                        available: false,
                        detail,
                    },
                    stdin_tx: None,
                    child_pid: None,
                    kill_tx: None,
                    exit_rx: None,
                    restart_task: None,
                    pending: HashMap::new(),
                }),
                handler: Mutex::new(None),
            }),
        }
    }

    /// Registers the handler and starts the bridge process if configured.
    pub fn start(&self, handler: Arc<dyn BridgeHandler>) {
        *self.inner.handler.lock().expect("bridge handler") = Some(handler);
        self.spawn_child();
    }

    pub fn status(&self) -> BridgeStatus {
        self.inner
            .state
            .lock()
            .expect("bridge state")
            .status
            .clone()
    }

    /// Sends an RPC request to the bridge and waits up to `timeout` for its answer.
    pub async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let (id, rx) = {
            let mut state = self.inner.state.lock().expect("bridge state");
            let has_child = state.stdin_tx.is_some();
            if !has_child || (!state.ready && method != "hello") {
                let message = if state.status.detail.is_empty() {
                    "The chat bridge is not running."
                } else {
                    &state.status.detail
                };
                return Err(AppError::with("bridgeUnavailable", 503, message));
            }
            state.next_id += 1;
            let id = format!("n{}", state.next_id);
            let (tx, rx) = oneshot::channel();
            state.pending.insert(id.clone(), tx);
            let frame = json!({ "id": id, "method": method, "params": params }).to_string();
            if let Some(stdin_tx) = &state.stdin_tx {
                let _ = stdin_tx.send(frame);
            }
            (id, rx)
        };

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(AppError::with(
                "bridgeUnavailable",
                503,
                "The chat bridge stopped.",
            )),
            Err(_) => {
                self.inner
                    .state
                    .lock()
                    .expect("bridge state")
                    .pending
                    .remove(&id);
                Err(AppError::with(
                    "bridgeTimeout",
                    504,
                    "The chat service did not answer in time.",
                ))
            }
        }
    }

    /// A one-way message the bridge never answers (microphone audio); dropped while it is down.
    pub fn notify(&self, method: &str, params: Value) {
        let state = self.inner.state.lock().expect("bridge state");
        if state.ready
            && let Some(stdin_tx) = &state.stdin_tx
        {
            let frame = json!({ "method": method, "params": params }).to_string();
            let _ = stdin_tx.send(frame);
        }
    }

    /// Stops the bridge process gracefully (stdin EOF + SIGTERM, then SIGKILL after 5s)
    /// and fails every pending request.
    pub async fn close(&self) {
        let (pid, exit_rx, kill_tx) = {
            let mut state = self.inner.state.lock().expect("bridge state");
            state.closing = true;
            state.ready = false;
            state.status.available = false;
            if let Some(handle) = state.restart_task.take() {
                handle.abort();
            }
            fail_all(&mut state, "Dar is stopping.");
            state.stdin_tx.take();
            (
                state.child_pid.take(),
                state.exit_rx.take(),
                state.kill_tx.take(),
            )
        };
        *self.inner.handler.lock().expect("bridge handler") = None;

        if let Some(pid) = pid {
            send_sigterm(pid);
        }
        if let Some(mut exit_rx) = exit_rx {
            let exited =
                tokio::time::timeout(Duration::from_secs(5), exit_rx.wait_for(|&done| done))
                    .await
                    .is_ok();
            if !exited {
                if let Some(tx) = kill_tx {
                    let _ = tx.send(());
                }
                let _ =
                    tokio::time::timeout(Duration::from_secs(2), exit_rx.wait_for(|&done| done))
                        .await;
            }
        }
    }

    fn spawn_child(&self) {
        let Some(executable) = self.inner.executable.as_deref() else {
            return;
        };
        {
            let state = self.inner.state.lock().expect("bridge state");
            if state.closing {
                return;
            }
        }

        let mut cmd = Command::new(executable);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env(
                "DAR_CHAT_BRIDGE_LOG",
                std::env::var("DAR_CHAT_BRIDGE_LOG").unwrap_or_else(|_| "warn".into()),
            )
            .env("TZ", "UTC");
        for (k, v) in &self.inner.env {
            cmd.env(k, v);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                let mut state = self.inner.state.lock().expect("bridge state");
                state.status.available = false;
                state.status.detail = if err.kind() == std::io::ErrorKind::NotFound {
                    "The chat bridge is not installed on this server.".to_owned()
                } else {
                    "The chat bridge could not be started.".to_owned()
                };
                return;
            }
        };

        let Some(mut stdin) = child.stdin.take() else {
            return;
        };
        let Some(stdout) = child.stdout.take() else {
            return;
        };
        let Some(stderr) = child.stderr.take() else {
            return;
        };

        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let (exit_tx, exit_rx) = watch::channel(false);

        let generation = {
            let mut state = self.inner.state.lock().expect("bridge state");
            if state.closing {
                let _ = child.start_kill();
                return;
            }
            state.generation += 1;
            state.ready = false;
            state.stdin_tx = Some(stdin_tx.clone());
            state.child_pid = child.id();
            state.kill_tx = Some(kill_tx);
            state.exit_rx = Some(exit_rx);
            state.generation
        };

        // Writer task: serializes all outgoing JSON lines to child stdin; closing stdin_tx closes stdin.
        tokio::spawn(async move {
            while let Some(mut line) = stdin_rx.recv().await {
                line.push('\n');
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // Stderr task: whatsmeow logs at warn and above; keep them, bounded per line.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let trimmed = line.trim_end_matches(['\r', '\n']);
                        if !trimmed.trim().is_empty() {
                            let bounded: String = trimmed.chars().take(500).collect();
                            eprintln!("chat-bridge: {bounded}");
                        }
                    }
                }
            }
        });

        // Stdout task: reads NDJSON frames (requests from bridge, events, and RPC responses).
        {
            let bridge = self.clone();
            let stdin_tx = stdin_tx.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let text = line.trim_end_matches(['\r', '\n']);
                            if text.is_empty() {
                                continue;
                            }
                            let Ok(Value::Object(mut frame)) = serde_json::from_str::<Value>(text)
                            else {
                                continue;
                            };

                            if let (Some(method), Some(id)) = (
                                frame
                                    .get("method")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                frame.get("id").and_then(Value::as_str).map(str::to_owned),
                            ) {
                                let params = frame
                                    .remove("params")
                                    .and_then(|v| match v {
                                        Value::Object(m) => Some(m),
                                        _ => None,
                                    })
                                    .unwrap_or_default();
                                let handler =
                                    bridge.inner.handler.lock().expect("bridge handler").clone();
                                let tx = stdin_tx.clone();
                                tokio::spawn(async move {
                                    let reply = match handler {
                                        Some(h) => match h.request(method, params).await {
                                            Ok(Value::Null) => json!({ "id": id, "result": {} }),
                                            Ok(value) => json!({ "id": id, "result": value }),
                                            Err(err) => json!({
                                                "id": id,
                                                "error": { "code": err.code, "message": err.message }
                                            }),
                                        },
                                        None => json!({
                                            "id": id,
                                            "error": { "code": "runtimeError", "message": "Request failed." }
                                        }),
                                    };
                                    let _ = tx.send(reply.to_string());
                                });
                                continue;
                            }

                            if let Some(event_name) = frame
                                .get("event")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                            {
                                let account = frame
                                    .get("account")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned();
                                let data = frame
                                    .remove("data")
                                    .and_then(|v| match v {
                                        Value::Object(m) => Some(m),
                                        _ => None,
                                    })
                                    .unwrap_or_default();
                                let handler =
                                    bridge.inner.handler.lock().expect("bridge handler").clone();
                                if let Some(h) = handler {
                                    h.event(account, event_name, data);
                                }
                                continue;
                            }

                            let Some(id) =
                                frame.get("id").and_then(Value::as_str).map(str::to_owned)
                            else {
                                continue;
                            };
                            let waiter = bridge
                                .inner
                                .state
                                .lock()
                                .expect("bridge state")
                                .pending
                                .remove(&id);
                            let Some(waiter) = waiter else {
                                continue;
                            };
                            if let Some(err_obj) = frame.get("error").and_then(Value::as_object) {
                                let code = err_obj
                                    .get("code")
                                    .and_then(Value::as_str)
                                    .unwrap_or("bridgeError");
                                let message = err_obj
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("The chat bridge reported an error.");
                                let status = map_bridge_error_status(code);
                                let _ = waiter.send(Err(AppError::with(code, status, message)));
                            } else {
                                let result = frame.remove("result").unwrap_or(Value::Null);
                                let _ = waiter.send(Ok(result));
                            }
                        }
                    }
                }
            });
        }

        // Exit supervisor task: waits for child exit (or forced kill) and schedules backoff restart.
        {
            let bridge = self.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = child.wait() => {}
                    _ = kill_rx => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }
                }
                let _ = exit_tx.send(true);

                let restart_delay = {
                    let mut state = bridge.inner.state.lock().expect("bridge state");
                    if state.generation == generation {
                        state.stdin_tx = None;
                        state.child_pid = None;
                        state.kill_tx = None;
                        state.exit_rx = None;
                    }
                    state.ready = false;
                    state.status.available = false;
                    fail_all(&mut state, "The chat bridge stopped.");
                    if state.closing
                        || state.status.detail == "The chat bridge is not installed on this server."
                    {
                        None
                    } else {
                        state.status.detail = "The chat bridge stopped; restarting.".to_owned();
                        let exp = state.restarts.min(6);
                        state.restarts = state.restarts.saturating_add(1);
                        let delay_ms = (1000u64 << exp).min(60_000);
                        Some(Duration::from_millis(delay_ms))
                    }
                };

                if let Some(delay) = restart_delay {
                    let bridge_for_restart = bridge.clone();
                    let handle = tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        bridge_for_restart.spawn_child();
                    });
                    let mut state = bridge.inner.state.lock().expect("bridge state");
                    if state.closing {
                        handle.abort();
                    } else {
                        if let Some(prev) = state.restart_task.replace(handle.abort_handle()) {
                            prev.abort();
                        }
                    }
                }
            });
        }

        // Hello handshake: verifies protocol version, marks bridge ready, and triggers `on_ready`.
        {
            let bridge = self.clone();
            tokio::spawn(async move {
                let Ok(hello) = bridge
                    .call("hello", json!({}), Duration::from_secs(10))
                    .await
                else {
                    return;
                };
                let protocol = hello.get("protocol").and_then(Value::as_i64);
                let mut mismatch_pid = None;
                let mut should_notify_ready = false;
                {
                    let mut state = bridge.inner.state.lock().expect("bridge state");
                    if state.closing || state.generation != generation {
                        return;
                    }
                    if protocol != Some(PROTOCOL_VERSION) {
                        state.status.detail =
                            "The chat bridge does not match this version of Dar.".to_owned();
                        mismatch_pid = state.child_pid;
                        state.stdin_tx.take();
                    } else {
                        state.ready = true;
                        state.restarts = 0;
                        state.status.available = true;
                        state.status.detail.clear();
                        should_notify_ready = true;
                    }
                }
                if let Some(pid) = mismatch_pid {
                    send_sigterm(pid);
                    return;
                }
                if should_notify_ready {
                    let handler = bridge.inner.handler.lock().expect("bridge handler").clone();
                    if let Some(h) = handler {
                        h.on_ready();
                    }
                }
            });
        }
    }
}

fn map_bridge_error_status(code: &str) -> u16 {
    match code {
        "invalidInput" => 400,
        "notConnected" | "unsupported" => 409,
        "forbidden" => 403,
        "notFound" => 404,
        "rateLimited" => 429,
        _ => 502,
    }
}

fn fail_all(state: &mut BridgeState, message: &str) {
    for (_, tx) in state.pending.drain() {
        let _ = tx.send(Err(AppError::with("bridgeUnavailable", 503, message)));
    }
}

fn send_sigterm(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
