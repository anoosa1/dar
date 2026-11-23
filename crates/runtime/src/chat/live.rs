//! One browser's live WebSocket (`next/runtime/chat/live.ts`): JSON text both ways,
//! Opus audio as binary, backpressure dropping late audio, keepalive pings every 30 s,
//! and idle timeout after 75 s.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value};
use tokio::sync::{Notify, mpsc};

use super::now_ms;

const MAX_MESSAGE: usize = 64 * 1024;
const BEHIND: usize = 256 * 1024;

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(1);

enum Outbound {
    Text(String),
    Audio(Vec<u8>),
    Close(u16, String),
}

/// Handle to one connected browser live socket. Cheap to clone and compare by `id`.
#[derive(Clone)]
pub(crate) struct LiveSocket {
    id: u64,
    closed: Arc<AtomicBool>,
    queued_audio_bytes: Arc<AtomicUsize>,
    tx: mpsc::UnboundedSender<Outbound>,
}

impl LiveSocket {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Sends a JSON text message if the socket is still open.
    pub fn send_text(&self, value: &Value) {
        if !self.closed.load(Ordering::Relaxed) {
            let _ = self.tx.send(Outbound::Text(value.to_string()));
        }
    }

    /// Sends Opus audio unless the browser is behind; late audio is worse than lost audio.
    pub fn send_audio(&self, data: Vec<u8>) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        let len = data.len();
        let queued = self.queued_audio_bytes.load(Ordering::Relaxed);
        if queued + len >= BEHIND {
            return;
        }
        self.queued_audio_bytes.fetch_add(len, Ordering::Relaxed);
        if self.tx.send(Outbound::Audio(data)).is_err() {
            self.queued_audio_bytes.fetch_sub(len, Ordering::Relaxed);
        }
    }

    /// Initiates a clean WebSocket close with `code` and `reason`.
    pub fn close(&self, code: u16, reason: &str) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let _ = self.tx.send(Outbound::Close(code, reason.to_owned()));
        }
    }
}

/// Callbacks for one live WebSocket's lifecycle.
pub(crate) trait LiveHandler: Send + Sync + 'static {
    fn on_text(&self, socket_id: u64, value: Map<String, Value>);
    fn on_binary(&self, socket_id: u64, data: &[u8]);
    fn on_close(&self, socket_id: u64);
}

/// Creates a `LiveSocket` handle and spawns the background reader/writer tasks for `ws`.
/// Callers can call `on_attach` (via the returned handle) before any incoming frame is dispatched
/// because the reader task waits on `start_rx` until registration finishes.
pub(crate) fn spawn_live_socket(
    ws: WebSocket,
    handler: Arc<dyn LiveHandler>,
) -> (LiveSocket, tokio::sync::oneshot::Sender<()>) {
    let id = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let closed = Arc::new(AtomicBool::new(false));
    let queued_audio_bytes = Arc::new(AtomicUsize::new(0));
    let last_seen = Arc::new(AtomicI64::new(now_ms()));
    let close_notify = Arc::new(Notify::new());
    let (tx, mut rx) = mpsc::unbounded_channel::<Outbound>();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();

    let handle = LiveSocket {
        id,
        closed: closed.clone(),
        queued_audio_bytes: queued_audio_bytes.clone(),
        tx,
    };

    let (mut sink, mut stream) = ws.split();

    // Writer + keepalive ping task.
    {
        let closed = closed.clone();
        let queued_audio_bytes = queued_audio_bytes.clone();
        let last_seen = last_seen.clone();
        let close_notify = close_notify.clone();
        tokio::spawn(async move {
            let mut ping = tokio::time::interval(Duration::from_secs(30));
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Skip immediate first tick.
            ping.tick().await;

            loop {
                tokio::select! {
                    outbound = rx.recv() => {
                        match outbound {
                            Some(Outbound::Text(text)) => {
                                if sink.send(Message::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                            Some(Outbound::Audio(data)) => {
                                let len = data.len();
                                let res = sink.send(Message::Binary(Bytes::from(data))).await;
                                queued_audio_bytes.fetch_sub(len, Ordering::Relaxed);
                                if res.is_err() {
                                    break;
                                }
                            }
                            Some(Outbound::Close(code, reason)) => {
                                let frame = CloseFrame {
                                    code,
                                    reason: reason.into(),
                                };
                                let _ = tokio::time::timeout(
                                    Duration::from_secs(1),
                                    sink.send(Message::Close(Some(frame))),
                                )
                                .await;
                                let _ = tokio::time::timeout(
                                    Duration::from_secs(1),
                                    sink.close(),
                                )
                                .await;
                                break;
                            }
                            None => break,
                        }
                    }
                    _ = ping.tick() => {
                        if now_ms() - last_seen.load(Ordering::Relaxed) > 75_000 {
                            closed.store(true, Ordering::Release);
                            let frame = CloseFrame {
                                code: 1001,
                                reason: "timeout".into(),
                            };
                            let _ = tokio::time::timeout(
                                Duration::from_secs(1),
                                sink.send(Message::Close(Some(frame))),
                            )
                            .await;
                            break;
                        }
                        if !closed.load(Ordering::Relaxed)
                            && sink.send(Message::Ping(Bytes::new())).await.is_err()
                        {
                            break;
                        }
                    }
                }
            }

            closed.store(true, Ordering::Release);
            close_notify.notify_one();
        });
    }

    // Reader task: waits for initial registration so hello/owner go out before handling frames.
    {
        let live = handle.clone();
        let last_seen = last_seen.clone();
        let close_notify = close_notify.clone();
        tokio::spawn(async move {
            let _ = start_rx.await;
            loop {
                let next = tokio::select! {
                    _ = close_notify.notified() => break,
                    msg = stream.next() => msg,
                };
                match next {
                    Some(Ok(msg)) => {
                        last_seen.store(now_ms(), Ordering::Relaxed);
                        match msg {
                            Message::Text(text) => {
                                if text.len() > MAX_MESSAGE {
                                    live.close(1009, "");
                                    break;
                                }
                                match serde_json::from_str::<Value>(&text) {
                                    Ok(Value::Object(map)) => handler.on_text(id, map),
                                    Ok(_) => {
                                        live.close(1003, "");
                                        break;
                                    }
                                    Err(_) => {
                                        live.close(1007, "");
                                        break;
                                    }
                                }
                            }
                            Message::Binary(data) => {
                                if data.len() > MAX_MESSAGE {
                                    live.close(1009, "");
                                    break;
                                }
                                handler.on_binary(id, &data);
                            }
                            Message::Ping(_) | Message::Pong(_) => {}
                            Message::Close(_) => {
                                live.close(1000, "");
                                break;
                            }
                        }
                    }
                    Some(Err(_)) | None => {
                        live.close(1000, "");
                        break;
                    }
                }
            }
            live.close(1000, "");
            handler.on_close(id);
        });
    }

    (handle, start_tx)
}
