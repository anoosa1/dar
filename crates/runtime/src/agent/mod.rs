//! Dar's Agent runtime (`next/runtime/agent/`): provider discovery, isolated workspaces,
//! Codex `app-server` and Claude Code `stream-json` runners, PTY terminals, and HTTP gateway.

pub mod app_server;
pub mod claude;
pub mod codex;
pub mod discovery;
pub mod http;
pub mod runtime;
pub mod terminal;
pub mod workspace;

pub use http::handle_http;
pub use runtime::AgentRuntime;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::error::{AppError, Result};

pub(crate) const AGENT_READ_OPERATIONS: [&str; 9] = [
    "source",
    "folders",
    "sharedThreads",
    "sharedHistory",
    "projects",
    "threads",
    "threadShares",
    "history",
    "run",
];

pub(crate) const HISTORY_READ_OPERATIONS: [&str; 4] =
    ["overview", "changeset", "content", "branch"];

#[cfg(any(target_arch = "aarch64", target_arch = "arm"))]
pub(crate) const O_NOFOLLOW: i32 = 0o100000;
#[cfg(not(any(target_arch = "aarch64", target_arch = "arm")))]
pub(crate) const O_NOFOLLOW: i32 = 0o400000;

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// Sends `SIGKILL` to the process group led by `pid` (equivalent to `process.kill(-pid, "SIGKILL")`).
pub(crate) fn kill_process_group(pid: u32) {
    if pid == 0 || pid > i32::MAX as u32 {
        return;
    }
    // SIGKILL is 9 on Linux across all architectures.
    unsafe {
        let _ = kill(-(pid as i32), 9);
    }
}

pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub(crate) fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

pub(crate) fn slice_utf16(s: &str, max_units: usize) -> &str {
    let mut units = 0;
    for (idx, ch) in s.char_indices() {
        let len = ch.len_utf16();
        if units + len > max_units {
            return &s[..idx];
        }
        units += len;
    }
    s
}

pub(crate) fn tail_utf16(s: &str, max_units: usize) -> &str {
    let mut units = 0;
    let mut start = s.len();
    for (idx, ch) in s.char_indices().rev() {
        let len = ch.len_utf16();
        if units + len > max_units {
            break;
        }
        units += len;
        start = idx;
    }
    &s[start..]
}

pub(crate) fn valid_effort(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase())
}

pub(crate) fn active_agent_run(status: &str) -> bool {
    matches!(status, "queued" | "running" | "cancelling")
}

pub(crate) fn agent_provider_of(model: &str) -> &'static str {
    if model.starts_with("claude/") {
        "claude"
    } else {
        "codex"
    }
}

pub(crate) fn agent_provider_name(provider: &str) -> &'static str {
    if provider == "claude" {
        "Claude Code"
    } else {
        "Codex"
    }
}

/// Cooperative cancellation token backed by a `tokio::sync::watch` channel.
#[derive(Clone)]
pub(crate) struct CancelToken {
    tx: Arc<watch::Sender<bool>>,
}

impl CancelToken {
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(false);
        Self { tx: Arc::new(tx) }
    }

    pub fn with_timeout(duration: Duration) -> Self {
        let token = Self::new();
        let copy = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            copy.cancel();
        });
        token
    }

    /// Stores the flag even when nothing awaits `cancelled` at the moment (`send` would drop it
    /// without a receiver, and a loop between two waits would never stop).
    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }

    pub fn throw_if_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(AppError::new("runStopped", 409))
        } else {
            Ok(())
        }
    }

    pub async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        if *rx.borrow_and_update() {
            return;
        }
        let _ = rx.wait_for(|v| *v).await;
    }
}

#[cfg(test)]
mod tests {
    use super::CancelToken;

    #[test]
    fn cancel_is_kept_without_a_waiter() {
        let token = CancelToken::new();
        let copy = token.clone();
        token.cancel();
        assert!(copy.is_cancelled());
        assert!(copy.throw_if_cancelled().is_err());
    }
}
