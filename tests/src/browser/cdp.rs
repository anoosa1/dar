//! Chrome DevTools Protocol over `--remote-debugging-pipe`: Chromium reads commands on fd 3 and
//! writes replies and events on fd 4, each a JSON message ended by a NUL byte. A reader thread
//! routes replies to their callers and events to the session (page) that subscribed, so any
//! tokio runtime can drive the browser.

use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// A protocol event: `session` is `None` for browser-level events (downloads, targets).
#[derive(Debug, Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session: Option<String>,
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>;

pub struct Connection {
    writer: Mutex<std::io::PipeWriter>,
    next: AtomicU64,
    pending: Arc<Pending>,
    sessions: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Event>>>>,
    browser: Arc<Mutex<Vec<mpsc::UnboundedSender<Event>>>>,
}

/// How long one protocol command may take before the test fails instead of hanging.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

impl Connection {
    pub fn new(writer: std::io::PipeWriter, reader: std::io::PipeReader) -> Arc<Self> {
        let connection = Arc::new(Self {
            writer: Mutex::new(writer),
            next: AtomicU64::new(1),
            pending: Arc::default(),
            sessions: Arc::default(),
            browser: Arc::default(),
        });
        let (pending, sessions, browser) = (
            connection.pending.clone(),
            connection.sessions.clone(),
            connection.browser.clone(),
        );
        std::thread::Builder::new()
            .name("cdp-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                let mut buffer = Vec::new();
                loop {
                    buffer.clear();
                    match reader.read_until(0, &mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    if buffer.last() == Some(&0) {
                        buffer.pop();
                    }
                    let Ok(message) = serde_json::from_slice::<Value>(&buffer) else {
                        continue;
                    };
                    if let Some(id) = message.get("id").and_then(Value::as_u64) {
                        let Some(reply) = pending.lock().unwrap().remove(&id) else {
                            continue;
                        };
                        let result = match message.get("error") {
                            Some(error) => Err(Error::new(format!(
                                "{}",
                                error
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("protocol error")
                            ))
                            .context(
                                error
                                    .get("data")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default(),
                            )),
                            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = reply.send(result);
                        continue;
                    }
                    let event = Event {
                        method: message["method"].as_str().unwrap_or_default().to_owned(),
                        params: message.get("params").cloned().unwrap_or(Value::Null),
                        session: message
                            .get("sessionId")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    };
                    match &event.session {
                        Some(session) => {
                            if let Some(sender) = sessions.lock().unwrap().get(session) {
                                let _ = sender.send(event);
                            }
                        }
                        None => browser
                            .lock()
                            .unwrap()
                            .retain(|sender| sender.send(event.clone()).is_ok()),
                    }
                }
                // The browser went away: fail whatever still waits for a reply.
                for (_, reply) in pending.lock().unwrap().drain() {
                    let _ = reply.send(Err(Error::new("the browser closed its protocol pipe")));
                }
            })
            .expect("CDP reader thread");
        connection
    }

    pub async fn call(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);
        {
            let mut bytes = serde_json::to_vec(&message)?;
            bytes.push(0);
            let mut writer = self.writer.lock().unwrap();
            if let Err(error) = writer.write_all(&bytes) {
                self.pending.lock().unwrap().remove(&id);
                return Err(Error::new(format!("{method}: {error}")));
            }
        }
        match tokio::time::timeout(COMMAND_TIMEOUT, receiver).await {
            Ok(Ok(result)) => result.map_err(|error| error.context(method)),
            Ok(Err(_)) => Err(Error::new(format!("{method}: the browser closed"))),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(Error::new(format!(
                    "{method}: no reply in {COMMAND_TIMEOUT:?}"
                )))
            }
        }
    }

    pub fn subscribe_session(&self, session: &str) -> mpsc::UnboundedReceiver<Event> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.sessions
            .lock()
            .unwrap()
            .insert(session.to_owned(), sender);
        receiver
    }

    pub fn unsubscribe_session(&self, session: &str) {
        self.sessions.lock().unwrap().remove(session);
    }

    pub fn subscribe_browser(&self) -> mpsc::UnboundedReceiver<Event> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.browser.lock().unwrap().push(sender);
        receiver
    }
}
