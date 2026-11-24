//! Interactive shells for a thread's working copy (`next/runtime/agent/terminal.ts`).
//! Each session is a PTY started through `codex app-server` `command/exec`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::agent::app_server::{
    AppServer, AppServerHandlers, Json, RequestFuture, open_app_server,
};
use crate::agent::codex::CodexConfig;
use crate::agent::utf16_len;
use crate::auth::uuid;
use crate::error::{AppError, Result};

const CAP: usize = 512 * 1024;
const IDLE_SECS: u64 = 15 * 60;

struct BufferState {
    chunks: VecDeque<(usize, Vec<u8>)>,
    start: usize,
    end: usize,
    exited: bool,
    exit_code: Option<i32>,
}

struct Session {
    server: AppServer,
    process_id: String,
    state: Arc<Mutex<BufferState>>,
    notify: Arc<Notify>,
    idle: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct Terminals {
    root: PathBuf,
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
}

fn clamp_size(value: Option<f64>, min: i64, max: i64) -> i64 {
    match value {
        Some(n) if n.is_finite() && n.fract() == 0.0 => (n as i64).clamp(min, max),
        _ => min,
    }
}

struct TerminalHandlers {
    process_id: String,
    state: Arc<Mutex<BufferState>>,
    notify: Arc<Notify>,
}

impl AppServerHandlers for TerminalHandlers {
    fn notify(&self, method: &str, params: Json) {
        if method == "command/exec/outputDelta"
            && params.get("processId").and_then(Value::as_str) == Some(self.process_id.as_str())
            && let Some(delta_b64) = params.get("deltaBase64").and_then(Value::as_str)
            && let Ok(data) = B64.decode(delta_b64)
        {
            let mut st = self.state.lock().expect("terminal state");
            let offset = st.end;
            st.end += data.len();
            st.chunks.push_back((offset, data));
            while st.end - st.start > CAP && st.chunks.len() > 1 {
                if let Some((off, dropped)) = st.chunks.pop_front() {
                    st.start = off + dropped.len();
                }
            }
            drop(st);
            self.notify.notify_waiters();
        }
    }

    fn request(&self, _method: String, _params: Json) -> RequestFuture {
        Box::pin(async { Err(AppError::new("declined", 403)) })
    }
}

impl Terminals {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn touch(&self, key: &str, session: &Arc<Session>) {
        let terminals = self.clone();
        let key_owned = key.to_owned();
        let mut idle = session.idle.lock().expect("idle");
        if let Some(handle) = idle.take() {
            handle.abort();
        }
        *idle = Some(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(IDLE_SECS)).await;
            terminals.close_key(&key_owned);
        }));
    }

    fn close_key(&self, key: &str) {
        let session = self.sessions.lock().expect("terminals").remove(key);
        if let Some(session) = session {
            if let Some(handle) = session.idle.lock().expect("idle").take() {
                handle.abort();
            }
            {
                let mut st = session.state.lock().expect("terminal state");
                st.exited = true;
            }
            session.notify.notify_waiters();
            session.server.close(None);
        }
    }

    pub async fn open(
        &self,
        config: &CodexConfig,
        user_id: &str,
        thread_id: &str,
        rows: Option<f64>,
        cols: Option<f64>,
    ) -> Result<Value> {
        let key = format!("{user_id}/{thread_id}");
        if let Some(existing) = self.sessions.lock().expect("terminals").get(&key).cloned() {
            let (exited, start, end) = {
                let st = existing.state.lock().expect("terminal state");
                (st.exited, st.start, st.end)
            };
            if !exited {
                self.touch(&key, &existing);
                return Ok(json!({ "start": start, "end": end }));
            }
            self.close_key(&key);
        }

        let workspace = self
            .root
            .join("agent-workspaces")
            .join(user_id)
            .join(thread_id)
            .join("work");
        let ok_dir = tokio::fs::symlink_metadata(&workspace)
            .await
            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
        if !ok_dir {
            return Err(AppError::with(
                "workspaceUnavailable",
                409,
                "Run Codex once in this thread to prepare its working copy, then open a terminal.",
            ));
        }

        let process_id = uuid();
        let state = Arc::new(Mutex::new(BufferState {
            chunks: VecDeque::new(),
            start: 0,
            end: 0,
            exited: false,
            exit_code: None,
        }));
        let notify = Arc::new(Notify::new());
        let server = open_app_server(
            config,
            &workspace,
            TerminalHandlers {
                process_id: process_id.clone(),
                state: state.clone(),
                notify: notify.clone(),
            },
            &["-c", "approval_policy=\"never\""],
        );

        let session = Arc::new(Session {
            server: server.clone(),
            process_id: process_id.clone(),
            state: state.clone(),
            notify: notify.clone(),
            idle: Mutex::new(None),
        });
        self.sessions
            .lock()
            .expect("terminals")
            .insert(key.clone(), session.clone());
        self.touch(&key, &session);

        let server_exit = server.clone();
        let state_exit = state.clone();
        let notify_exit = notify.clone();
        tokio::spawn(async move {
            let _ = server_exit.exited().await;
            {
                let mut st = state_exit.lock().expect("terminal state");
                st.exited = true;
            }
            notify_exit.notify_waiters();
        });

        let init = server
            .request(
                "initialize",
                json!({
                    "clientInfo": { "name": "dar", "title": "Dar", "version": "1.0.0" },
                    "capabilities": { "experimentalApi": true },
                }),
            )
            .await;
        if let Err(err) = init {
            self.close_key(&key);
            return Err(err);
        }
        server.notify("initialized", json!({}));

        let clamped_rows = clamp_size(rows, 4, 200);
        let clamped_cols = clamp_size(cols, 20, 400);
        let ws_str = workspace.to_string_lossy().into_owned();
        let exec_server = server.clone();
        let exec_state = state.clone();
        let exec_notify = notify.clone();
        tokio::spawn(async move {
            let res = exec_server
                .request(
                    "command/exec",
                    json!({
                        "command": ["/bin/sh", "-i"],
                        "processId": process_id,
                        "tty": true,
                        "disableTimeout": true,
                        "cwd": ws_str,
                        "env": { "TERM": "xterm-256color", "PS1": "$ " },
                        "size": { "rows": clamped_rows, "cols": clamped_cols },
                    }),
                )
                .await;
            {
                let mut st = exec_state.lock().expect("terminal state");
                if let Ok(val) = res
                    && let Some(code) = val.get("exitCode").and_then(Value::as_i64)
                {
                    st.exit_code = Some(code as i32);
                }
                st.exited = true;
            }
            exec_notify.notify_waiters();
        });

        Ok(json!({ "start": 0, "end": 0 }))
    }

    /// Returns bytes after `after`, waiting up to 15 seconds for new output.
    pub async fn read(&self, user_id: &str, thread_id: &str, after: usize) -> Value {
        let key = format!("{user_id}/{thread_id}");
        let Some(session) = self.sessions.lock().expect("terminals").get(&key).cloned() else {
            return json!({ "open": false, "start": 0, "end": 0, "data": "", "exited": true });
        };
        self.touch(&key, &session);

        // Register notification before checking condition to prevent missed wakeups.
        let notified = session.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let should_wait = {
            let st = session.state.lock().expect("terminal state");
            after >= st.end && !st.exited
        };
        if should_wait {
            let _ = tokio::time::timeout(Duration::from_secs(15), notified).await;
        }

        let (start, end, exited, exit_code, bytes) = {
            let st = session.state.lock().expect("terminal state");
            let from = after.max(st.start);
            let mut out = Vec::new();
            for (offset, data) in &st.chunks {
                if offset + data.len() > from {
                    let skip = from.saturating_sub(*offset);
                    out.extend_from_slice(&data[skip..]);
                }
            }
            (st.start, st.end, st.exited, st.exit_code, out)
        };

        let mut value = json!({
            "open": true,
            "start": start,
            "end": end,
            "data": B64.encode(&bytes),
            "exited": exited,
        });
        if let Some(code) = exit_code {
            value["exitCode"] = json!(code);
        }
        value
    }

    pub async fn write(&self, user_id: &str, thread_id: &str, data: &str) -> Result<Value> {
        let key = format!("{user_id}/{thread_id}");
        let session = self.sessions.lock().expect("terminals").get(&key).cloned();
        let Some(session) = session else {
            return Err(AppError::with(
                "terminalClosed",
                409,
                "The terminal is closed.",
            ));
        };
        if session.state.lock().expect("terminal state").exited {
            return Err(AppError::with(
                "terminalClosed",
                409,
                "The terminal is closed.",
            ));
        }
        if utf16_len(data) > 65_536 {
            return Err(AppError::new("requestTooLarge", 413));
        }
        self.touch(&key, &session);
        session
            .server
            .request(
                "command/exec/write",
                json!({
                    "processId": session.process_id,
                    "deltaBase64": B64.encode(data.as_bytes()),
                }),
            )
            .await?;
        Ok(json!({ "ok": true }))
    }

    pub async fn resize(
        &self,
        user_id: &str,
        thread_id: &str,
        rows: Option<f64>,
        cols: Option<f64>,
    ) -> Value {
        let key = format!("{user_id}/{thread_id}");
        let session = self.sessions.lock().expect("terminals").get(&key).cloned();
        let Some(session) = session else {
            return json!({ "ok": false });
        };
        if session.state.lock().expect("terminal state").exited {
            return json!({ "ok": false });
        }
        let _ = session
            .server
            .request(
                "command/exec/resize",
                json!({
                    "processId": session.process_id,
                    "size": {
                        "rows": clamp_size(rows, 4, 200),
                        "cols": clamp_size(cols, 20, 400),
                    },
                }),
            )
            .await;
        json!({ "ok": true })
    }

    pub fn close(&self, user_id: &str, thread_id: &str) -> Value {
        self.close_key(&format!("{user_id}/{thread_id}"));
        json!({ "ok": true })
    }

    pub fn close_all(&self) {
        let keys: Vec<String> = self
            .sessions
            .lock()
            .expect("terminals")
            .keys()
            .cloned()
            .collect();
        for key in keys {
            self.close_key(&key);
        }
    }
}
