//! Agent runtime: provider status, connections, usage limits, Codex & Claude Code run
//! execution, steering, blocking questions, attachments, local Git, history branches,
//! file mentions and terminals (`next/runtime/agent/runtime.ts`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::{oneshot, watch};

use crate::agent::app_server::{
    AppServerHandlers, Json, RequestFuture, assert_no_codex_rules, open_app_server,
};
use crate::agent::claude::{
    ClaudeConfig, ClaudeHandlers, ClaudeProcess, ClaudeRunOptions, ControlRequestFuture,
    claude_account, claude_args, claude_commands_available, claude_limits, claude_models,
    open_claude,
};
use crate::agent::codex::{
    CODEX_INSTRUCTIONS, CodexConfig, codex_account, codex_models, object, verify_codex_sandbox,
};
use crate::agent::discovery::{codex_search_directories, find_codex_executable};
use crate::agent::terminal::Terminals;
use crate::agent::workspace::{
    Source, branch_changes, git, prepare_workspace, scan,
};
use crate::agent::{
    CancelToken, HISTORY_READ_OPERATIONS, active_agent_run, agent_provider_name,
    agent_provider_of, home_dir, slice_utf16, tail_utf16, utf16_len,
};
use crate::auth::{is_uuid, uuid};
use crate::blobs::Blobs;
use crate::db::Database;
use crate::error::{AppError, Result};
use crate::http::locale_compare;
use crate::root::create_durable_file;

pub const AGENT_RUN_MINUTES: u64 = 30;
const AGENT_ATTACHMENT_MAX_COUNT: usize = 4;
const AGENT_ATTACHMENT_MAX_BYTES: u64 = 8 * 1024 * 1024;
const AGENT_IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAttachment {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StepItem {
    text: String,
    done: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentStep {
    id: String,
    kind: String,
    label: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Vec<StepItem>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AgentUsage {
    input: u64,
    cached: u64,
    output: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct QuestionOption {
    label: String,
    description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AgentQuestion {
    id: String,
    header: String,
    question: String,
    options: Vec<QuestionOption>,
    other: bool,
    secret: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AgentPending {
    id: String,
    questions: Vec<AgentQuestion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRunDocument {
    output: String,
    terminal: String,
    activity: Vec<String>,
    changes: Vec<Value>,
    error: String,
    truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    steps: Option<Vec<AgentStep>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<AgentUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<AgentAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    steered: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<Option<AgentPending>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex_thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claude_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_used: Option<u64>,
}

impl AgentRunDocument {
    fn blank() -> Self {
        Self {
            output: String::new(),
            terminal: String::new(),
            activity: Vec::new(),
            changes: Vec::new(),
            error: String::new(),
            truncated: false,
            steps: None,
            usage: None,
            effort: None,
            mode: None,
            attachments: None,
            steered: None,
            pending: None,
            codex_thread: None,
            claude_session: None,
            context_revision: None,
            context_window: None,
            context_used: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentMessageRow {
    sequence: u32,
    role: String,
    content: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentThreadRow {
    project_id: String,
    title: String,
    revision: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRunSummaryRow {
    id: String,
    status: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentHistoryRow {
    thread: AgentThreadRow,
    items: Vec<AgentMessageRow>,
    next_before: Option<u32>,
    runs: Vec<AgentRunSummaryRow>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimedRunRow {
    owner_id: String,
    thread_id: String,
    model: String,
    document: String,
}

/// Small insertion-order preserving map (matching JS `Map`).
struct OrderedMap<V> {
    entries: Vec<(String, V)>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        Self { entries: Vec::new() }
    }
}

impl<V> OrderedMap<V> {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    fn get(&self, key: &str) -> Option<&V> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn insert(&mut self, key: String, value: V) {
        if let Some((_, existing)) = self.entries.iter_mut().find(|(k, _)| k == &key) {
            *existing = value;
        } else {
            self.entries.push((key, value));
        }
    }

    fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }
}

/// The module accepts at most 512 KiB per save; step output and the terminal tail give way first.
fn bounded(data: &AgentRunDocument) -> AgentRunDocument {
    let fits = |value: &AgentRunDocument| -> bool {
        let serialized = json!({ "status": "completed", "document": value }).to_string();
        utf16_len(&serialized) <= 500 * 1024
    };
    if fits(data) {
        return data.clone();
    }
    let mut next = data.clone();
    next.truncated = true;
    if let Some(ref mut steps) = next.steps {
        for step in steps.iter_mut() {
            step.output = None;
        }
    }
    if fits(&next) {
        return next;
    }
    next.terminal = tail_utf16(&next.terminal, 8192).to_owned();
    for change in next.changes.iter_mut() {
        if let Some(obj) = change.as_object_mut() {
            obj.remove("content");
            obj.remove("fileId");
            obj.remove("expectedRevision");
            obj.remove("mediaType");
            if let Some(diff) = obj.get("diff").and_then(Value::as_str).map(str::to_owned) {
                obj.insert("diff".into(), json!(slice_utf16(&diff, 4096)));
            }
        }
    }
    if fits(&next) {
        return next;
    }
    if let Some(steps) = next.steps.take() {
        let start = steps.len().saturating_sub(20);
        let trimmed: Vec<AgentStep> = steps[start..]
            .iter()
            .map(|step| {
                let mut s = step.clone();
                s.label = slice_utf16(&s.label, 500).to_owned();
                s
            })
            .collect();
        next.steps = Some(trimmed);
    }
    next.output = slice_utf16(&next.output, 32_768).to_owned();
    next.terminal.clear();
    next
}

/// Why a run stopped itself, in words the run owner can act on.
fn stop_reason(error: &AppError) -> String {
    match error.code.as_str() {
        "workspaceTooLarge" => "The working copy grew past Dar's limits (200 files, 1 MiB per file, 8 MiB total), so the run was stopped. Remove generated files such as dependencies or build output, then retry.".into(),
        "unsafeWorkspace" => "The working copy now contains a symbolic link or special file, so the run was stopped. Remove it, then retry.".into(),
        "workspaceChanged" => "A working copy file kept changing while Dar checked it, so the run was stopped. Retry when the command has finished.".into(),
        "unauthenticated" => "Your session ended while the run was working, so it was stopped. Sign in again and retry.".into(),
        "forbidden" => "The run lost access to its project (the folder was moved or unshared, or the Agent was turned off), so it was stopped.".into(),
        "operationTooLarge" | "invalidInput" => "Dar could not save this run's progress, so it was stopped. Its output so far is kept.".into(),
        "databaseUnavailable" | "" => "Dar could not reach its database for several seconds, so the run was stopped. Retry when Dar is less busy.".into(),
        code => {
            if error.message != code {
                error.message.clone()
            } else {
                format!("The run stopped unexpectedly ({code}).")
            }
        }
    }
}

/// Codex and Claude Code bindings share one shape: paths, account ID and Git hosts, never tokens.
async fn configuration(filename: &Path, name: &str) -> Result<Option<CodexConfig>> {
    let stat = match tokio::fs::symlink_metadata(filename).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(AppError::with(
                "codexConfigurationInvalid",
                503,
                &format!("{name} configuration is invalid. Ask the administrator to check the executable, {name} home and Dar account ID."),
            ));
        }
    };
    let invalid = || {
        AppError::with(
            "codexConfigurationInvalid",
            503,
            &format!("{name} configuration is invalid. Ask the administrator to check the executable, {name} home and Dar account ID."),
        )
    };
    if !stat.is_file() || stat.file_type().is_symlink() || stat.len() > 16_384 {
        return Err(invalid());
    }
    let text = tokio::fs::read_to_string(filename)
        .await
        .map_err(|_| invalid())?;
    let parsed: Value = serde_json::from_str(&text).map_err(|_| invalid())?;
    let value = object(parsed).map_err(|_| invalid())?;
    if value
        .keys()
        .any(|k| !["executable", "home", "userId", "gitHosts"].contains(&k.as_str()))
    {
        return Err(invalid());
    }
    let executable = value
        .get("executable")
        .and_then(Value::as_str)
        .filter(|s| Path::new(s).is_absolute())
        .ok_or_else(invalid)?
        .to_owned();
    let home = value
        .get("home")
        .and_then(Value::as_str)
        .filter(|s| Path::new(s).is_absolute())
        .ok_or_else(invalid)?
        .to_owned();
    let user_id = value
        .get("userId")
        .and_then(Value::as_str)
        .filter(|s| is_uuid(s))
        .ok_or_else(invalid)?
        .to_owned();
    let hosts_arr = value
        .get("gitHosts")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    let mut git_hosts = Vec::with_capacity(hosts_arr.len());
    for host in hosts_arr {
        let h = host.as_str().ok_or_else(invalid)?;
        if h.is_empty()
            || !h
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            return Err(invalid());
        }
        git_hosts.push(h.to_ascii_lowercase());
    }
    let home_stat = tokio::fs::symlink_metadata(&home)
        .await
        .map_err(|_| invalid())?;
    if !home_stat.is_dir() || home_stat.file_type().is_symlink() {
        return Err(invalid());
    }
    Ok(Some(CodexConfig {
        executable,
        home,
        user_id,
        git_hosts,
    }))
}

type DeliverFn = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;
type SteerFn = Arc<dyn Fn(String) + Send + Sync>;

struct PendingAnswer {
    id: String,
    tx: oneshot::Sender<HashMap<String, Vec<String>>>,
}

struct ActiveRun {
    cancel: CancelToken,
    session_id: String,
    done_rx: watch::Receiver<bool>,
    turn: Mutex<Option<String>>,
    answer: Mutex<Option<PendingAnswer>>,
    steered: Mutex<HashSet<String>>,
    steer_fn: Mutex<Option<SteerFn>>,
    deliver_fn: Mutex<Option<DeliverFn>>,
}

struct ProviderStatus {
    id: &'static str,
    name: &'static str,
    configured: bool,
    connected: bool,
    commands: Option<bool>,
    message: String,
}

impl ProviderStatus {
    fn to_json(&self) -> Value {
        let mut obj = json!({
            "id": self.id,
            "name": self.name,
            "configured": self.configured,
            "connected": self.connected,
            "message": self.message,
        });
        if let Some(c) = self.commands {
            obj["commands"] = json!(c);
        }
        obj
    }
}

struct CheckedProvider {
    config: Option<CodexConfig>,
    status: ProviderStatus,
    models: Vec<Value>,
}

struct ConnectionCheck {
    codex: CheckedProvider,
    claude: CheckedProvider,
    provider: Value,
}

struct RunDocState {
    data: AgentRunDocument,
    dirty: bool,
    messages: OrderedMap<String>,
    outputs: OrderedMap<String>,
    steps: OrderedMap<AgentStep>,
}

impl RunDocState {
    fn new(data: AgentRunDocument) -> Self {
        Self {
            data,
            dirty: true,
            messages: OrderedMap::default(),
            outputs: OrderedMap::default(),
            steps: OrderedMap::default(),
        }
    }

    fn limit(&mut self, value: &str, max: usize) -> String {
        if utf16_len(value) > max {
            self.data.truncated = true;
        }
        slice_utf16(value, max).to_owned()
    }

    fn text(&mut self, value: Option<&Value>, max: usize) -> String {
        match value.and_then(Value::as_str) {
            Some(s) => self.limit(s, max),
            None => String::new(),
        }
    }

    fn remember_message(&mut self, id: String, value: &str) {
        if self.messages.len() >= 100 && !self.messages.contains_key(&id) {
            self.data.truncated = true;
            return;
        }
        let limited = self.limit(value, 65_536);
        self.messages.insert(id, limited);
    }

    fn remember_output(&mut self, id: String, value: &str) {
        if self.outputs.len() >= 100 && !self.outputs.contains_key(&id) {
            self.data.truncated = true;
            return;
        }
        let limited = self.limit(value, 65_536);
        self.outputs.insert(id, limited);
    }

    fn record_step(&mut self, step: AgentStep) {
        if !self.steps.contains_key(&step.id) && self.steps.len() >= 100 {
            self.data.truncated = true;
            return;
        }
        self.steps.insert(step.id.clone(), step);
        self.data.steps = Some(self.steps.values().cloned().collect());
        self.dirty = true;
    }

    fn refresh_output(&mut self) {
        let joined_msg = self
            .messages
            .values()
            .filter(|v| !v.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n");
        let joined_term = self
            .outputs
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        self.data.output = self.limit(&joined_msg, 65_536);
        self.data.terminal = self.limit(&joined_term, 65_536);
        self.dirty = true;
    }

    fn push_activity(&mut self, entry: String) {
        self.data.activity.push(entry);
        if self.data.activity.len() > 100 {
            let drain = self.data.activity.len() - 100;
            self.data.activity.drain(..drain);
        }
        self.dirty = true;
    }
}

fn count_u64(value: Option<&Value>) -> u64 {
    match value.and_then(Value::as_f64) {
        Some(n) if n.is_finite() && n.fract() == 0.0 && n >= 0.0 && n <= 9_007_199_254_740_991.0 => {
            n as u64
        }
        _ => 0,
    }
}

fn safe_i64(value: Option<&Value>) -> Option<i64> {
    match value.and_then(Value::as_f64) {
        Some(n) if n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0 => {
            Some(n as i64)
        }
        _ => None,
    }
}

fn format_messages(items: &[AgentMessageRow]) -> String {
    items
        .iter()
        .map(|m| {
            format!(
                "{}:\n{}",
                if m.role == "user" { "User" } else { "Assistant" },
                m.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[derive(Clone)]
pub struct AgentRuntime {
    db: Database,
    root: PathBuf,
    codex_file: PathBuf,
    claude_file: PathBuf,
    closing: Arc<AtomicBool>,
    terminals: Terminals,
    active: Arc<Mutex<HashMap<String, Arc<ActiveRun>>>>,
}

impl AgentRuntime {
    pub async fn open(
        db: Database,
        root: &Path,
        codex_file: PathBuf,
        claude_file: PathBuf,
    ) -> std::io::Result<Self> {
        db.call("service_agent_worker", json!(["", "", "startup", ""]))
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self {
            db,
            root: root.to_path_buf(),
            codex_file,
            claude_file,
            closing: Arc::new(AtomicBool::new(false)),
            terminals: Terminals::new(root.to_path_buf()),
            active: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.terminals.close_all();
        let runs: Vec<Arc<ActiveRun>> = self
            .active
            .lock()
            .expect("active")
            .values()
            .cloned()
            .collect();
        for run in &runs {
            run.cancel.cancel();
        }
        for run in runs {
            let mut rx = run.done_rx.clone();
            if !*rx.borrow_and_update() {
                let _ = rx.wait_for(|v| *v).await;
            }
        }
    }

    pub fn cancel_all(&self) {
        let runs: Vec<Arc<ActiveRun>> = self
            .active
            .lock()
            .expect("active")
            .values()
            .cloned()
            .collect();
        for run in runs {
            run.cancel.cancel();
        }
    }

    pub fn cancel_session(&self, session_id: &str) {
        let runs: Vec<Arc<ActiveRun>> = self
            .active
            .lock()
            .expect("active")
            .values()
            .filter(|r| r.session_id == session_id)
            .cloned()
            .collect();
        for run in runs {
            run.cancel.cancel();
        }
    }

    async fn operation(&self, session_id: &str, name: &str, value: Value) -> Result<Value> {
        self.db
            .call("service_agent", json!([session_id, name, value.to_string()]))
            .await
    }

    async fn worker(&self, id: &str, lease: &str, name: &str, value: Value) -> Result<Value> {
        self.db
            .call(
                "service_agent_worker",
                json!([id, lease, name, value.to_string()]),
            )
            .await
    }

    async fn status_workspace(&self) -> Result<PathBuf> {
        let workspace = self.root.join("agent-workspaces").join("provider-status");
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&workspace)
            .await
            .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
        let meta = tokio::fs::symlink_metadata(&workspace)
            .await
            .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
        if meta.file_type().is_symlink() {
            return Err(AppError::new("unsafeWorkspace", 409));
        }
        Ok(workspace)
    }

    async fn codex_connection(&self, user_id: &str) -> CheckedProvider {
        let unavailable = |config: Option<CodexConfig>, message: &str, configured: bool| CheckedProvider {
            config,
            status: ProviderStatus {
                id: "codex",
                name: "Codex",
                configured,
                connected: false,
                commands: None,
                message: message.to_owned(),
            },
            models: Vec::new(),
        };
        let config = match configuration(&self.codex_file, "Codex").await {
            Ok(Some(c)) => c,
            Ok(None) => {
                return unavailable(
                    None,
                    "No server Codex login is linked to this Dar account. An administrator can use Connect server Codex, or configure services.dar.agent.",
                    false,
                );
            }
            Err(_) => {
                return unavailable(
                    None,
                    "Codex configuration is invalid. Ask the administrator to check the executable, Codex home and Dar account ID, then refresh.",
                    false,
                );
            }
        };
        if config.user_id != user_id {
            return unavailable(
                Some(config),
                "The server Codex login is linked to a different Dar account. Ask the administrator to check the account binding.",
                false,
            );
        }
        let Ok(workspace) = self.status_workspace().await else {
            return unavailable(
                Some(config),
                "Dar could not run the configured Codex CLI. Check the executable and service permissions, then refresh.",
                true,
            );
        };
        match codex_account(&config, &workspace, None).await {
            Ok(true) => {}
            Ok(false) => {
                return unavailable(
                    Some(config),
                    "Codex is configured, but its ChatGPT login is unavailable to the Dar service. Sign in as the Linux account running Dar using the configured Codex home, then refresh.",
                    true,
                );
            }
            Err(_) => {
                return unavailable(
                    Some(config),
                    "Dar could not run the configured Codex CLI. Check the executable and service permissions, then refresh.",
                    true,
                );
            }
        }
        match codex_models(&config, &workspace, None).await {
            Ok(models) if !models.is_empty() => {
                let model_values: Vec<Value> = models
                    .into_iter()
                    .map(|m| {
                        let mut v = serde_json::to_value(m).unwrap_or(json!({}));
                        v["provider"] = json!("codex");
                        v
                    })
                    .collect();
                CheckedProvider {
                    config: Some(config),
                    status: ProviderStatus {
                        id: "codex",
                        name: "Codex",
                        configured: true,
                        connected: true,
                        commands: Some(true),
                        message: "Connected with the existing Codex CLI ChatGPT login.".into(),
                    },
                    models: model_values,
                }
            }
            Ok(_) => unavailable(
                Some(config),
                "Codex is signed in, but the CLI returned no available models. Check the installed Codex version, then refresh.",
                true,
            ),
            Err(_) => unavailable(
                Some(config),
                "Codex is signed in, but Dar could not read its model catalog. Check the installed Codex version, then refresh.",
                true,
            ),
        }
    }

    async fn claude_connection(&self, user_id: &str) -> CheckedProvider {
        let unavailable = |config: Option<ClaudeConfig>, message: &str, configured: bool| CheckedProvider {
            config,
            status: ProviderStatus {
                id: "claude",
                name: "Claude Code",
                configured,
                connected: false,
                commands: None,
                message: message.to_owned(),
            },
            models: Vec::new(),
        };
        let config = match configuration(&self.claude_file, "Claude Code").await {
            Ok(Some(c)) => c,
            Ok(None) => {
                return unavailable(
                    None,
                    "No server Claude Code login is linked to this Dar account. An administrator can use Connect server Claude Code, or configure services.dar.agent.claude.",
                    false,
                );
            }
            Err(_) => {
                return unavailable(
                    None,
                    "Claude Code configuration is invalid. Ask the administrator to check the executable, Claude home and Dar account ID, then refresh.",
                    false,
                );
            }
        };
        if config.user_id != user_id {
            return unavailable(
                Some(config),
                "The server Claude Code login is linked to a different Dar account. Ask the administrator to check the account binding.",
                false,
            );
        }
        let Ok(workspace) = self.status_workspace().await else {
            return unavailable(
                Some(config),
                "Dar could not run the configured Claude Code CLI. Check the executable and service permissions, then refresh.",
                true,
            );
        };
        let account = match claude_account(&config, &workspace, None).await {
            Ok(Some(acc)) => acc,
            Ok(None) => {
                return unavailable(
                    Some(config),
                    "Claude Code is configured, but its Claude login is unavailable to the Dar service. Sign in with claude as the Linux account running Dar using the configured Claude home, then refresh.",
                    true,
                );
            }
            Err(_) => {
                return unavailable(
                    Some(config),
                    "Dar could not run the configured Claude Code CLI. Check the executable and service permissions, then refresh.",
                    true,
                );
            }
        };
        match claude_models(&config, &workspace, None).await {
            Ok(models) if !models.is_empty() => {
                let commands = claude_commands_available().await;
                let plan_suffix = if account.plan.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", account.plan)
                };
                let cmd_suffix = if commands {
                    ""
                } else {
                    " Commands are off until bubblewrap and socat are on the service PATH; Claude can still read and edit files."
                };
                CheckedProvider {
                    config: Some(config),
                    status: ProviderStatus {
                        id: "claude",
                        name: "Claude Code",
                        configured: true,
                        connected: true,
                        commands: Some(commands),
                        message: format!(
                            "Connected with the existing Claude Code login{plan_suffix}.{cmd_suffix}"
                        ),
                    },
                    models,
                }
            }
            Ok(_) => unavailable(
                Some(config),
                "Claude Code is signed in, but the CLI returned no available models. Check the installed version, then refresh.",
                true,
            ),
            Err(_) => unavailable(
                Some(config),
                "Claude Code is signed in, but Dar could not read its model catalog. Check the installed version, then refresh.",
                true,
            ),
        }
    }

    async fn connection(&self, user_id: &str) -> ConnectionCheck {
        let (codex, claude) =
            tokio::join!(self.codex_connection(user_id), self.claude_connection(user_id));
        let all = [&codex, &claude];
        let usable: Vec<&CheckedProvider> =
            all.iter().copied().filter(|item| item.status.connected).collect();
        let configured: Vec<&CheckedProvider> = all
            .iter()
            .copied()
            .filter(|item| item.status.configured)
            .collect();
        let message = if !usable.is_empty() {
            usable
                .iter()
                .map(|i| i.status.message.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        } else if !configured.is_empty() {
            configured
                .iter()
                .map(|i| i.status.message.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            "No server Codex or Claude Code login is linked to this Dar account. An administrator can connect one in Agent settings, or configure services.dar.agent.".into()
        };
        let models: Vec<Value> = usable
            .iter()
            .flat_map(|item| item.models.iter().cloned())
            .collect();
        let providers: Vec<Value> = all.iter().map(|item| item.status.to_json()).collect();
        let provider = json!({
            "configured": !configured.is_empty(),
            "connected": !usable.is_empty(),
            "models": models,
            "message": message,
            "providers": providers,
        });
        ConnectionCheck {
            codex,
            claude,
            provider,
        }
    }

    pub async fn status(&self, user_id: &str) -> Value {
        self.connection(user_id).await.provider
    }

    async fn codex_limits(&self, config: &CodexConfig) -> Value {
        struct DeclineHandlers;
        impl AppServerHandlers for DeclineHandlers {
            fn notify(&self, _method: &str, _params: Json) {}
            fn request(&self, _method: String, _params: Json) -> RequestFuture {
                Box::pin(async { Err(AppError::new("declined", 403)) })
            }
        }

        let workspace = match self.status_workspace().await {
            Ok(w) => w,
            Err(e) => {
                return json!({ "available": false, "plan": null, "windows": [], "message": e.message });
            }
        };
        let server = open_app_server(
            config,
            &workspace,
            DeclineHandlers,
            &["-c", "approval_policy=\"never\""],
        );
        let srv_timer = server.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            srv_timer.close(None);
        });

        let result: Result<Value> = async {
            assert_no_codex_rules(config).await?;
            server
                .request(
                    "initialize",
                    json!({
                        "clientInfo": { "name": "dar", "title": "Dar", "version": "1.0.0" },
                        "capabilities": { "experimentalApi": true },
                    }),
                )
                .await?;
            server.notify("initialized", json!({}));
            let value = server.request("account/rateLimits/read", json!({})).await?;
            let snapshots: Vec<&Map<String, Value>> = if let Some(by_id) = value
                .get("rateLimitsByLimitId")
                .and_then(Value::as_object)
            {
                by_id.values().filter_map(Value::as_object).take(6).collect()
            } else {
                value
                    .get("rateLimits")
                    .and_then(Value::as_object)
                    .into_iter()
                    .collect()
            };
            let finite = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|n| n.is_finite());
            let mut windows = Vec::new();
            let mut plan: Option<String> = None;
            for snapshot in snapshots {
                if plan.is_none() {
                    plan = snapshot
                        .get("planType")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                let name = snapshot
                    .get("limitName")
                    .and_then(Value::as_str)
                    .or_else(|| snapshot.get("limitId").and_then(Value::as_str))
                    .unwrap_or("Codex");
                for (key, fallback_label) in [
                    ("primary", "Short window"),
                    ("secondary", "Weekly window"),
                ] {
                    if let Some(window) = snapshot.get(key).and_then(Value::as_object)
                        && let Some(used) = finite(window.get("usedPercent"))
                    {
                        let minutes = finite(window.get("windowDurationMins"));
                        let resets = finite(window.get("resetsAt")).map(|r| {
                            if r < 1e12 { r * 1000.0 } else { r }
                        });
                        let span_label = match minutes {
                            Some(m) if m != 0.0 => {
                                if m >= 1440.0 {
                                    format!("{} day", (m / 1440.0).round() as i64)
                                } else {
                                    format!("{} hour", (m / 60.0).round() as i64)
                                }
                            }
                            _ => fallback_label.to_owned(),
                        };
                        windows.push(json!({
                            "label": format!("{name} \u{00b7} {span_label}"),
                            "usedPercent": used.clamp(0.0, 100.0),
                            "windowMinutes": minutes,
                            "resetsAt": resets,
                        }));
                    }
                }
            }
            Ok(json!({ "available": true, "plan": plan, "windows": windows }))
        }
        .await;

        timer.abort();
        server.close(None);
        match result {
            Ok(v) => v,
            Err(e) => {
                json!({ "available": false, "plan": null, "windows": [], "message": e.message })
            }
        }
    }

    pub async fn limits(&self, user_id: &str) -> Value {
        let checked = self.connection(user_id).await;
        if checked.provider["connected"] != Value::Bool(true) {
            return json!({
                "available": false,
                "plan": null,
                "windows": [],
                "message": checked.provider["message"],
            });
        }
        let mut parts: Vec<(&'static str, Value)> = Vec::new();
        if checked.codex.status.connected
            && let Some(ref config) = checked.codex.config
        {
            parts.push(("codex", self.codex_limits(config).await));
        }
        if checked.claude.status.connected
            && let Some(ref config) = checked.claude.config
        {
            let claude_part = match self.status_workspace().await {
                Ok(ws) => match claude_limits(config, &ws).await {
                    Ok(v) => v,
                    Err(e) => json!({ "available": false, "plan": null, "windows": [], "message": e.message }),
                },
                Err(e) => json!({ "available": false, "plan": null, "windows": [], "message": e.message }),
            };
            parts.push(("claude", claude_part));
        }
        let ready: Vec<&(&'static str, Value)> = parts
            .iter()
            .filter(|(_, part)| part["available"] == Value::Bool(true))
            .collect();
        if ready.is_empty() {
            let msg = parts
                .iter()
                .filter_map(|(_, part)| part["message"].as_str())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            return json!({
                "available": false,
                "plan": null,
                "windows": [],
                "message": if msg.is_empty() { "Usage limits are unavailable.".to_owned() } else { msg },
            });
        }
        let plans: Vec<(&str, &str)> = ready
            .iter()
            .filter_map(|(name, part)| part["plan"].as_str().map(|p| (*name, p)))
            .collect();
        let plan: Option<String> = if plans.len() > 1 {
            Some(
                plans
                    .iter()
                    .map(|(name, p)| format!("{} {p}", agent_provider_name(name)))
                    .collect::<Vec<_>>()
                    .join(" \u{00b7} "),
            )
        } else {
            plans.first().map(|(_, p)| (*p).to_owned())
        };
        let windows: Vec<Value> = ready
            .iter()
            .flat_map(|(_, part)| {
                part["windows"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        json!({ "available": true, "plan": plan, "windows": windows })
    }

    async fn connect_codex(&self, user_id: &str) -> Result<Value> {
        if let Some(existing) = configuration(&self.codex_file, "Codex").await? {
            if existing.user_id != user_id {
                return Err(AppError::with(
                    "codexAlreadyLinked",
                    409,
                    "Codex is already linked to another Dar account. The administrator must update its server configuration.",
                ));
            }
            return Ok(self.connection(user_id).await.provider);
        }
        let home_path = std::env::var("CODEX_HOME")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".codex"));
        let path_env = std::env::var("PATH").unwrap_or_default();
        let executable = find_codex_executable(&codex_search_directories(&path_env, &home_dir()), "codex")
            .await
            .ok_or_else(|| {
                AppError::with(
                    "codexUnavailable",
                    503,
                    "Dar could not access Codex on its service PATH or standard installation paths. A private SSH-user installation may be hidden from the service. Set services.dar.agent.package on NixOS, or set the absolute executable path in codex.json.",
                )
            })?;
        let home_ok = home_path.is_absolute()
            && tokio::fs::symlink_metadata(&home_path)
                .await
                .is_ok_and(|s| s.is_dir() && !s.file_type().is_symlink());
        if !home_ok {
            return Err(AppError::with(
                "codexLoginRequired",
                409,
                "The Dar service has no accessible Codex home. Sign in to Codex as the Linux account running Dar, then connect again.",
            ));
        }
        let discovered = CodexConfig {
            executable: executable.to_string_lossy().into_owned(),
            home: home_path.to_string_lossy().into_owned(),
            user_id: user_id.to_owned(),
            git_hosts: vec!["github.com".into()],
        };
        let workspace = self.status_workspace().await?;
        if !codex_account(&discovered, &workspace, None).await? {
            return Err(AppError::with(
                "codexLoginRequired",
                409,
                "No ChatGPT login is available to the Dar service. Run codex login as the Linux account running Dar, then connect again.",
            ));
        }
        let bytes = serde_json::to_vec(&discovered).map_err(|_| AppError::internal())?;
        let target = self.codex_file.clone();
        let res = tokio::task::spawn_blocking(move || create_durable_file(&target, &bytes))
            .await
            .map_err(|_| AppError::internal())?;
        if let Err(err) = res
            && err.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(AppError::with(
                "codexConfigurationReadOnly",
                409,
                "Dar cannot save the Codex binding. Configure services.dar.agent or make the private configuration directory writable by the Dar service.",
            ));
        }
        Ok(self.connection(user_id).await.provider)
    }

    async fn connect_claude(&self, user_id: &str) -> Result<Value> {
        if let Some(existing) = configuration(&self.claude_file, "Claude Code").await? {
            if existing.user_id != user_id {
                return Err(AppError::with(
                    "claudeAlreadyLinked",
                    409,
                    "Claude Code is already linked to another Dar account. The administrator must update its server configuration.",
                ));
            }
            return Ok(self.connection(user_id).await.provider);
        }
        let home_path = std::env::var("CLAUDE_CONFIG_DIR")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".claude"));
        let path_env = std::env::var("PATH").unwrap_or_default();
        let executable = find_codex_executable(&codex_search_directories(&path_env, &home_dir()), "claude")
            .await
            .ok_or_else(|| {
                AppError::with(
                    "claudeUnavailable",
                    503,
                    "Dar could not access Claude Code on its service PATH or standard installation paths. A private SSH-user installation may be hidden from the service. Set services.dar.agent.claude.package on NixOS, or set the absolute executable path in claude.json.",
                )
            })?;
        let home_ok = home_path.is_absolute()
            && tokio::fs::symlink_metadata(&home_path)
                .await
                .is_ok_and(|s| s.is_dir() && !s.file_type().is_symlink());
        if !home_ok {
            return Err(AppError::with(
                "claudeLoginRequired",
                409,
                "The Dar service has no accessible Claude home. Sign in to Claude Code as the Linux account running Dar, then connect again.",
            ));
        }
        let discovered = ClaudeConfig {
            executable: executable.to_string_lossy().into_owned(),
            home: home_path.to_string_lossy().into_owned(),
            user_id: user_id.to_owned(),
            git_hosts: vec!["github.com".into()],
        };
        let workspace = self.status_workspace().await?;
        if claude_account(&discovered, &workspace, None).await?.is_none() {
            return Err(AppError::with(
                "claudeLoginRequired",
                409,
                "No Claude login is available to the Dar service. Run claude and sign in as the Linux account running Dar, then connect again.",
            ));
        }
        let bytes = serde_json::to_vec(&discovered).map_err(|_| AppError::internal())?;
        let target = self.claude_file.clone();
        let res = tokio::task::spawn_blocking(move || create_durable_file(&target, &bytes))
            .await
            .map_err(|_| AppError::internal())?;
        if let Err(err) = res
            && err.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(AppError::with(
                "claudeConfigurationReadOnly",
                409,
                "Dar cannot save the Claude Code binding. Configure services.dar.agent.claude or make the private configuration directory writable by the Dar service.",
            ));
        }
        Ok(self.connection(user_id).await.provider)
    }

    /// Administrator-only server CLI connection action.
    pub async fn connect(&self, user_id: &str, provider: &str) -> Result<Value> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(AppError::new("codexUnavailable", 503));
        }
        if provider == "claude" {
            self.connect_claude(user_id).await
        } else {
            self.connect_codex(user_id).await
        }
    }

    async fn attachment_info(
        &self,
        user_id: &str,
        thread_id: &str,
        attachment_id: &str,
    ) -> Result<AgentAttachment> {
        if !is_uuid(attachment_id) {
            return Err(AppError::invalid());
        }
        let path = self
            .root
            .join("agent-workspaces")
            .join(user_id)
            .join(thread_id)
            .join("attachments")
            .join(format!("{attachment_id}.json"));
        let Ok(text) = tokio::fs::read_to_string(&path).await else {
            return Err(AppError::with(
                "notFound",
                404,
                "That attachment is not available.",
            ));
        };
        let Ok(info) = serde_json::from_str::<AgentAttachment>(&text) else {
            return Err(AppError::with(
                "notFound",
                404,
                "That attachment is not available.",
            ));
        };
        if info.id != attachment_id {
            return Err(AppError::with(
                "notFound",
                404,
                "That attachment is not available.",
            ));
        }
        Ok(info)
    }

    pub async fn start(
        &self,
        session_id: &str,
        user_id: &str,
        mut p: Map<String, Value>,
    ) -> Result<Value> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(AppError::new("codexUnavailable", 503));
        }
        let model_str = p.get("model").and_then(Value::as_str).unwrap_or("");
        let which = agent_provider_of(model_str);
        let checked = self.connection(user_id).await;
        let chosen = if which == "claude" {
            &checked.claude
        } else {
            &checked.codex
        };
        let Some(config) = chosen.config.clone() else {
            return Err(AppError::with(
                &format!("{which}NotConfigured"),
                409,
                &chosen.status.message,
            ));
        };
        if !chosen.status.configured {
            return Err(AppError::with(
                &format!("{which}NotConfigured"),
                409,
                &chosen.status.message,
            ));
        }
        if !chosen.status.connected {
            return Err(AppError::with(
                &format!("{which}LoginRequired"),
                409,
                &chosen.status.message,
            ));
        }
        let model = chosen
            .models
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(model_str))
            .ok_or_else(|| {
                AppError::with(
                    "invalidModel",
                    400,
                    &format!("Choose one of the available {} models.", agent_provider_name(which)),
                )
            })?;
        if which == "claude" && p.get("mode").and_then(Value::as_str) == Some("compact") {
            return Err(AppError::with(
                "compactUnavailable",
                409,
                "Claude Code compacts its context automatically when it is needed.",
            ));
        }
        if let Some(effort_val) = p.get("effort") {
            let effort_str = effort_val.as_str().unwrap_or("");
            let supported = model
                .get("efforts")
                .and_then(Value::as_array)
                .is_some_and(|arr| arr.iter().any(|e| e.as_str() == Some(effort_str)));
            if !supported || effort_val.as_str().is_none() {
                return Err(AppError::with(
                    "invalidEffort",
                    400,
                    "Choose a reasoning level this model supports.",
                ));
            }
        }
        if let Some(att_val) = p.get("attachments") {
            let Some(list) = att_val.as_array() else {
                return Err(AppError::invalid());
            };
            if list.len() > AGENT_ATTACHMENT_MAX_COUNT {
                return Err(AppError::invalid());
            }
            let mut seen = HashSet::new();
            let thread_id = p
                .get("threadId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let mut resolved = Vec::with_capacity(list.len());
            for item in list {
                let raw = match item {
                    Value::String(s) => s.clone(),
                    _ => return Err(AppError::invalid()),
                };
                if !seen.insert(raw.clone()) {
                    return Err(AppError::invalid());
                }
                let info = self.attachment_info(user_id, &thread_id, &raw).await?;
                resolved.push(serde_json::to_value(info).map_err(|_| AppError::internal())?);
            }
            p.insert("attachments".into(), Value::Array(resolved));
        }

        let run = self
            .operation(session_id, "startRun", Value::Object(p))
            .await?;
        let run_id = run.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
        if run.get("status").and_then(Value::as_str) == Some("queued") && !run_id.is_empty() {
            let (entry, should_spawn, done_tx) = {
                let mut active = self.active.lock().expect("active");
                if active.contains_key(&run_id) {
                    (None, false, None)
                } else {
                    let (done_tx, done_rx) = watch::channel(false);
                    let entry = Arc::new(ActiveRun {
                        cancel: CancelToken::new(),
                        session_id: session_id.to_owned(),
                        done_rx,
                        turn: Mutex::new(None),
                        answer: Mutex::new(None),
                        steered: Mutex::new(HashSet::new()),
                        steer_fn: Mutex::new(None),
                        deliver_fn: Mutex::new(None),
                    });
                    active.insert(run_id.clone(), entry.clone());
                    (Some(entry), true, Some(done_tx))
                }
            };
            if should_spawn
                && let (Some(entry), Some(done_tx)) = (entry, done_tx)
            {
                let runtime = self.clone();
                let sid = session_id.to_owned();
                tokio::spawn(async move {
                    runtime
                        .execute(run_id, sid, entry, config, which, done_tx)
                        .await;
                });
            }
        }
        Ok(run)
    }

    async fn execute(
        self,
        id: String,
        session_id: String,
        entry: Arc<ActiveRun>,
        config: CodexConfig,
        provider: &'static str,
        done_tx: watch::Sender<bool>,
    ) {
        let lease = uuid();
        let state = Arc::new(Mutex::new(RunDocState::new(AgentRunDocument::blank())));
        let failure_reason: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let cancel = entry.cancel.clone();

        let stop = {
            let cancel = cancel.clone();
            let failure_reason = failure_reason.clone();
            move |reason: String| {
                if cancel.is_cancelled() {
                    return;
                }
                {
                    let mut guard = failure_reason.lock().expect("failure");
                    if guard.is_none() {
                        *guard = Some(reason);
                    }
                }
                cancel.cancel();
            }
        };

        let stop_timeout = stop.clone();
        let timeout_handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(AGENT_RUN_MINUTES * 60)).await;
            stop_timeout(format!(
                "The run reached Dar's {AGENT_RUN_MINUTES} minute limit and was stopped. Its output so far is kept."
            ));
        });

        let finish = |status: &'static str, state: Arc<Mutex<RunDocState>>| {
            let this = self.clone();
            let id = id.clone();
            let lease = lease.clone();
            async move {
                let doc_snapshot = state.lock().expect("state").data.clone();
                let bounded_doc = bounded(&doc_snapshot);
                match this
                    .worker(
                        &id,
                        &lease,
                        "finish",
                        json!({ "status": status, "document": bounded_doc }),
                    )
                    .await
                {
                    Ok(_) => return,
                    Err(err)
                        if matches!(err.code.as_str(), "forbidden" | "conflict" | "notFound") =>
                    {
                        let _ = this.worker(&id, &lease, "interrupt", json!({})).await;
                        return;
                    }
                    Err(_) => {}
                }
                let mut minimal = AgentRunDocument::blank();
                minimal.output = slice_utf16(&doc_snapshot.output, 16_000).to_owned();
                let err_text = if doc_snapshot.error.is_empty() {
                    "The run finished, but its full details could not be saved."
                } else {
                    &doc_snapshot.error
                };
                minimal.error = slice_utf16(err_text, 2000).to_owned();
                minimal.truncated = true;
                minimal.claude_session = doc_snapshot.claude_session;
                minimal.codex_thread = doc_snapshot.codex_thread;
                minimal.context_revision = doc_snapshot.context_revision;
                minimal.effort = doc_snapshot.effort;
                minimal.mode = doc_snapshot.mode;
                minimal.attachments = doc_snapshot.attachments;
                let fallback_status = if status == "completed" {
                    "failed"
                } else {
                    status
                };
                if this
                    .worker(
                        &id,
                        &lease,
                        "finish",
                        json!({ "status": fallback_status, "document": minimal }),
                    )
                    .await
                    .is_err()
                {
                    let _ = this.worker(&id, &lease, "interrupt", json!({})).await;
                }
            }
        };

        let checkpoint_stop = CancelToken::new();
        let workspace_to_validate: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let mut checkpoint_task: Option<tokio::task::JoinHandle<()>> = None;

        let run_result: Result<&'static str> = async {
            let claimed = self.worker(&id, &lease, "claim", json!({})).await?;
            if claimed.is_null() {
                return Ok("__unclaimed__");
            }
            let row: ClaimedRunRow =
                serde_json::from_value(claimed).map_err(|_| AppError::internal())?;
            let initial: AgentRunDocument =
                serde_json::from_str(&row.document).map_err(|_| AppError::internal())?;
            {
                let mut st = state.lock().expect("state");
                st.data.effort = initial.effort;
                st.data.mode = initial.mode;
                st.data.attachments = initial.attachments;
            }

            // Start 500ms checkpoint & lease heartbeat loop.
            {
                let cp_this = self.clone();
                let cp_id = id.clone();
                let cp_lease = lease.clone();
                let cp_state = state.clone();
                let cp_ws = workspace_to_validate.clone();
                let cp_stop = checkpoint_stop.clone();
                let cp_abort = stop.clone();
                checkpoint_task = Some(tokio::spawn(async move {
                    let mut strikes: u32 = 0;
                    let mut last_ws_check: Option<Instant> = None;
                    loop {
                        tokio::select! {
                            _ = cp_stop.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                        }
                        if cp_stop.is_cancelled() {
                            break;
                        }
                        let tick_res: Result<()> = async {
                            cp_this.worker(&cp_id, &cp_lease, "check", json!({})).await?;
                            let ws_opt = cp_ws.lock().expect("ws").clone();
                            if let Some(ws) = ws_opt
                                && last_ws_check.is_none_or(|t| t.elapsed() > Duration::from_secs(2))
                            {
                                last_ws_check = Some(Instant::now());
                                let _ = scan(&ws).await?;
                            }
                            let maybe_doc = {
                                let mut st = cp_state.lock().expect("state");
                                if st.dirty {
                                    st.dirty = false;
                                    Some(bounded(&st.data))
                                } else {
                                    None
                                }
                            };
                            if let Some(doc) = maybe_doc {
                                cp_this
                                    .worker(&cp_id, &cp_lease, "update", json!({ "document": doc }))
                                    .await?;
                            }
                            Ok(())
                        }
                        .await;
                        match tick_res {
                            Ok(()) => strikes = 0,
                            Err(err) => {
                                if err.code == "databaseUnavailable" && strikes + 1 < 20 {
                                    strikes += 1;
                                    cp_state.lock().expect("state").dirty = true;
                                } else {
                                    cp_abort(stop_reason(&err));
                                    break;
                                }
                            }
                        }
                    }
                }));
            }

            let history: AgentHistoryRow = serde_json::from_value(
                self.operation(&session_id, "history", json!({ "threadId": row.thread_id }))
                    .await?,
            )
            .map_err(|_| AppError::internal())?;
            let source: Source = serde_json::from_value(
                self.operation(
                    &session_id,
                    "source",
                    json!({ "projectId": history.thread.project_id }),
                )
                .await?,
            )
            .map_err(|_| AppError::internal())?;

            if history.next_before.is_some() {
                return Err(AppError::with(
                    "contextTooLarge",
                    413,
                    "This thread has more than 50 messages. Start a new thread to provide bounded context.",
                ));
            }
            let full_context = format_messages(&history.items);
            if full_context.len() > 128 * 1024 {
                return Err(AppError::with(
                    "contextTooLarge",
                    413,
                    "The conversation is too large for this run. Start a new thread with a shorter task.",
                ));
            }

            let prepared = prepare_workspace(
                &self.root,
                &row.owner_id,
                &row.thread_id,
                source,
                &config,
                &self.db,
                &session_id,
                cancel.clone(),
            )
            .await?;
            cancel.throw_if_cancelled()?;
            *workspace_to_validate.lock().expect("ws") = Some(prepared.workspace.clone());

            // Continue the saved provider thread when possible; only unseen messages are sent.
            let mut previous: Option<AgentRunDocument> = None;
            for summary in &history.runs {
                if summary.id == id || active_agent_run(&summary.status) {
                    continue;
                }
                if let Ok(earlier_val) = self
                    .operation(&session_id, "run", json!({ "id": summary.id }))
                    .await
                    && let Ok(doc) =
                        serde_json::from_value::<AgentRunDocument>(earlier_val["document"].clone())
                {
                    let has_session = if provider == "claude" {
                        doc.claude_session.is_some()
                    } else {
                        doc.codex_thread.is_some()
                    };
                    if has_session && doc.context_revision.is_some() {
                        previous = Some(doc);
                    }
                }
                break;
            }
            let unseen: Vec<AgentMessageRow> = match &previous {
                Some(prev) => {
                    let rev = prev.context_revision.unwrap_or(0);
                    history
                        .items
                        .iter()
                        .filter(|m| m.sequence > rev)
                        .cloned()
                        .collect()
                }
                None => history.items.clone(),
            };

            let attachment_items = state
                .lock()
                .expect("state")
                .data
                .attachments
                .clone()
                .unwrap_or_default();
            let mut attachment_paths = Vec::with_capacity(attachment_items.len());
            for item in &attachment_items {
                let p = self
                    .root
                    .join("agent-workspaces")
                    .join(&row.owner_id)
                    .join(&row.thread_id)
                    .join("attachments")
                    .join(&item.id);
                let is_file = tokio::fs::symlink_metadata(&p)
                    .await
                    .is_ok_and(|m| m.is_file());
                if !is_file {
                    return Err(AppError::with(
                        "attachmentMissing",
                        409,
                        "An attached image is no longer available. Attach it again.",
                    ));
                }
                attachment_paths.push((item.clone(), p));
            }

            let status = if provider == "claude" {
                self.run_claude_turn(
                    &config,
                    &row,
                    &history,
                    &unseen,
                    previous.as_ref(),
                    &prepared,
                    &attachment_paths,
                    state.clone(),
                    entry.clone(),
                    cancel.clone(),
                )
                .await?
            } else {
                self.run_codex_turn(
                    &config,
                    &row,
                    &history,
                    &unseen,
                    previous.as_ref(),
                    &prepared,
                    &attachment_paths,
                    state.clone(),
                    entry.clone(),
                    cancel.clone(),
                )
                .await?
            };

            let final_history: AgentHistoryRow = serde_json::from_value(
                self.operation(&session_id, "history", json!({ "threadId": row.thread_id }))
                    .await?,
            )
            .map_err(|_| AppError::internal())?;
            {
                let mut st = state.lock().expect("state");
                let bonus = if status == "completed" && !st.data.output.is_empty() {
                    1
                } else {
                    0
                };
                st.data.context_revision = Some(final_history.thread.revision + bonus);
            }

            checkpoint_stop.cancel();
            if let Some(handle) = checkpoint_task.take() {
                let _ = handle.await;
            }
            cancel.throw_if_cancelled()?;

            let (changes, truncated) = prepared.changes().await?;
            {
                let mut st = state.lock().expect("state");
                st.data.changes = changes;
                st.data.truncated |= truncated;
                if status != "completed" {
                    st.data.error = if provider == "claude" {
                        let combined = if st.data.error.is_empty() {
                            "Claude Code could not complete the turn. Inspect its saved output before retrying.".to_owned()
                        } else {
                            format!(
                                "{}\n\nClaude Code could not complete the turn. Inspect its saved output before retrying.",
                                st.data.error
                            )
                        };
                        st.limit(&combined, 2000)
                    } else {
                        "The Codex CLI could not complete the turn. Inspect its saved output before retrying.".to_owned()
                    };
                }
            }
            finish(status, state.clone()).await;
            Ok(status)
        }
        .await;

        checkpoint_stop.cancel();
        if let Some(handle) = checkpoint_task.take() {
            let _ = handle.await;
        }
        timeout_handle.abort();

        if let Err(err) = run_result {
            let recorded_failure = failure_reason.lock().expect("failure").clone();
            if !cancel.is_cancelled() || recorded_failure.is_some() {
                let msg = recorded_failure.unwrap_or_else(|| stop_reason(&err));
                state.lock().expect("state").data.error = slice_utf16(&msg, 2000).to_owned();
                finish("failed", state.clone()).await;
            } else {
                let _ = self.worker(&id, &lease, "interrupt", json!({})).await;
            }
        }

        self.active.lock().expect("active").remove(&id);
        let _ = done_tx.send(true);
    }

    async fn ask_user(
        entry: Arc<ActiveRun>,
        state: Arc<Mutex<RunDocState>>,
        cancel: CancelToken,
        questions: Vec<AgentQuestion>,
    ) -> Result<HashMap<String, Vec<String>>> {
        let key = uuid();
        let (tx, rx) = oneshot::channel();
        {
            let mut st = state.lock().expect("state");
            st.data.pending = Some(Some(AgentPending {
                id: key.clone(),
                questions: questions.clone(),
            }));
            st.dirty = true;
        }
        {
            let mut ans = entry.answer.lock().expect("answer");
            *ans = Some(PendingAnswer {
                id: key.clone(),
                tx,
            });
        }
        let answers = tokio::select! {
            res = rx => res.map_err(|_| AppError::new("runStopped", 409))?,
            _ = cancel.cancelled() => return Err(AppError::new("runStopped", 409)),
        };
        {
            let mut st = state.lock().expect("state");
            st.data.pending = Some(None);
            for question in &questions {
                let raw_answers: Vec<String> = if question.secret {
                    vec!["Answered".to_owned()]
                } else {
                    answers.get(&question.id).cloned().unwrap_or_default()
                };
                let items: Vec<StepItem> = raw_answers
                    .into_iter()
                    .take(50)
                    .map(|a| {
                        let limited = st.limit(&a, 500);
                        StepItem {
                            text: limited,
                            done: true,
                        }
                    })
                    .collect();
                let label = if !question.question.is_empty() {
                    question.question.clone()
                } else if !question.header.is_empty() {
                    question.header.clone()
                } else {
                    "Question".to_owned()
                };
                st.record_step(AgentStep {
                    id: format!("question-{key}-{}", question.id),
                    kind: "question".into(),
                    label,
                    status: "completed".into(),
                    exit_code: None,
                    output: None,
                    items: Some(items),
                });
            }
        }
        Ok(answers)
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_claude_turn(
        &self,
        claude_config: &ClaudeConfig,
        row: &ClaimedRunRow,
        history: &AgentHistoryRow,
        unseen: &[AgentMessageRow],
        previous: Option<&AgentRunDocument>,
        prepared: &crate::agent::workspace::PreparedWorkspace,
        attachments: &[(AgentAttachment, PathBuf)],
        state: Arc<Mutex<RunDocState>>,
        entry: Arc<ActiveRun>,
        cancel: CancelToken,
    ) -> Result<&'static str> {
        if claude_account(claude_config, &prepared.workspace, Some(cancel.clone()))
            .await?
            .is_none()
        {
            return Err(AppError::with(
                "claudeLoginRequired",
                409,
                "The configured Claude Code login is no longer available.",
            ));
        }
        let commands = claude_commands_available().await;
        let model = row
            .model
            .strip_prefix("claude/")
            .unwrap_or(&row.model)
            .to_owned();

        let mut deny_seen = HashSet::new();
        let mut deny = Vec::new();
        let candidates = [
            "/home".to_owned(),
            "/root".to_owned(),
            "/var/lib".to_owned(),
            "/srv".to_owned(),
            "/mnt".to_owned(),
            "/media".to_owned(),
            "/run/secrets".to_owned(),
            "/run/credentials".to_owned(),
            "/run/keys".to_owned(),
            self.root.to_string_lossy().into_owned(),
            claude_config.home.clone(),
            home_dir().to_string_lossy().into_owned(),
        ];
        for candidate in candidates {
            if deny_seen.insert(candidate.clone())
                && tokio::fs::symlink_metadata(&candidate).await.is_ok()
            {
                deny.push(candidate);
            }
        }

        let mut images = Vec::with_capacity(attachments.len());
        for (item, path) in attachments {
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|_| AppError::new("attachmentMissing", 409))?;
            images.push(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": item.media_type,
                    "data": B64.encode(bytes),
                }
            }));
        }

        let (effort, mode) = {
            let st = state.lock().expect("state");
            (st.data.effort.clone(), st.data.mode.clone())
        };
        let mut input = if previous.and_then(|p| p.claude_session.as_ref()).is_some() {
            format_messages(unseen)
        } else {
            format_messages(&history.items)
        };
        if mode.as_deref() == Some("review") {
            let (changes, _) = prepared.changes().await?;
            let diff = changes
                .iter()
                .filter_map(|c| c.get("diff").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n");
            let trimmed = slice_utf16(&diff, 96 * 1024);
            let body = if trimmed.is_empty() {
                "No files have changed."
            } else {
                trimmed
            };
            input = format!(
                "Review the uncommitted changes in this working copy against its starting point. Report bugs, risks and missing tests by file, most severe first. Do not edit files.\n\n{body}"
            );
        }

        let workspace_prefix = format!("{}/", prepared.workspace.to_string_lossy());

        struct AttemptOutcome {
            initialized: bool,
            status: Option<&'static str>,
            diagnostics: String,
        }

        struct TurnHandlers {
            state: Arc<Mutex<RunDocState>>,
            entry: Arc<ActiveRun>,
            cancel: CancelToken,
            thread_revision: u32,
            workspace_prefix: String,
            initialized: Arc<AtomicBool>,
            status: Arc<Mutex<Option<&'static str>>>,
            proc_slot: Arc<Mutex<Option<ClaudeProcess>>>,
            stream_state: Mutex<(String, String, HashMap<String, usize>, HashMap<String, usize>)>,
        }

        impl TurnHandlers {
            fn relative(&self, st: &mut RunDocState, value: Option<&Value>) -> String {
                let full = st.text(value, 2000);
                if let Some(rest) = full.strip_prefix(&self.workspace_prefix) {
                    rest.to_owned()
                } else {
                    full
                }
            }
        }

        impl ClaudeHandlers for TurnHandlers {
            fn message(&self, m: Json) {
                if m.get("parent_tool_use_id")
                    .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
                {
                    return;
                }
                let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "system" => {
                        let subtype = m.get("subtype").and_then(Value::as_str).unwrap_or("");
                        if subtype == "init"
                            && let Some(sid) = m.get("session_id").and_then(Value::as_str)
                            && is_uuid(sid)
                        {
                            self.initialized.store(true, Ordering::SeqCst);
                            let mut st = self.state.lock().expect("state");
                            st.data.claude_session = Some(sid.to_owned());
                            st.data.context_revision = Some(self.thread_revision);
                            st.dirty = true;
                        } else if subtype == "compact_boundary" {
                            let mut st = self.state.lock().expect("state");
                            st.record_step(AgentStep {
                                id: format!("compacted-{}", uuid()),
                                kind: "tool".into(),
                                label: "Compacted the conversation context".into(),
                                status: "completed".into(),
                                exit_code: None,
                                output: None,
                                items: None,
                            });
                        }
                    }
                    "stream_event" => {
                        let Some(event) = m.get("event").and_then(Value::as_object) else {
                            return;
                        };
                        let etype = event.get("type").and_then(Value::as_str).unwrap_or("");
                        let mut ss = self.stream_state.lock().expect("stream");
                        let mut st = self.state.lock().expect("state");
                        match etype {
                            "message_start" => {
                                let msg_id = event
                                    .get("message")
                                    .and_then(Value::as_object)
                                    .and_then(|msg| msg.get("id"));
                                ss.0 = st.text(msg_id, 200);
                            }
                            "content_block_start" => {
                                let is_text = event
                                    .get("content_block")
                                    .and_then(Value::as_object)
                                    .and_then(|b| b.get("type"))
                                    .and_then(Value::as_str)
                                    == Some("text");
                                if is_text && !ss.0.is_empty() {
                                    let current = ss.0.clone();
                                    let n = *ss.2.get(&current).unwrap_or(&0);
                                    ss.2.insert(current.clone(), n + 1);
                                    ss.1 = format!("{current}:{n}");
                                }
                            }
                            "content_block_delta" => {
                                let delta = event.get("delta").and_then(Value::as_object);
                                if let Some(d) = delta
                                    && d.get("type").and_then(Value::as_str) == Some("text_delta")
                                    && !ss.1.is_empty()
                                    && let Some(txt) = d.get("text").and_then(Value::as_str)
                                {
                                    let key = ss.1.clone();
                                    let prev = st.messages.get(&key).cloned().unwrap_or_default();
                                    let next = format!("{prev}{txt}");
                                    st.remember_message(key, &next);
                                    st.refresh_output();
                                }
                            }
                            "content_block_stop" => {
                                ss.1.clear();
                            }
                            _ => {}
                        }
                    }
                    "assistant" => {
                        let message = m.get("message").and_then(Value::as_object);
                        let Some(message) = message else { return };
                        let mut ss = self.stream_state.lock().expect("stream");
                        let mut st = self.state.lock().expect("state");
                        let raw_mid = st.text(message.get("id"), 200);
                        let message_id = if raw_mid.is_empty() { uuid() } else { raw_mid };
                        if let Some(usage) = message.get("usage").and_then(Value::as_object) {
                            let used = count_u64(usage.get("input_tokens"))
                                + count_u64(usage.get("cache_read_input_tokens"))
                                + count_u64(usage.get("cache_creation_input_tokens"))
                                + count_u64(usage.get("output_tokens"));
                            if used > 0 {
                                st.data.context_used = Some(used);
                                st.dirty = true;
                            }
                        }
                        if let Some(blocks) = message.get("content").and_then(Value::as_array) {
                            for block_val in blocks {
                                let Some(block) = block_val.as_object() else {
                                    continue;
                                };
                                let btype = block.get("type").and_then(Value::as_str).unwrap_or("");
                                if btype == "text"
                                    && let Some(txt) = block.get("text").and_then(Value::as_str)
                                {
                                    let n = *ss.3.get(&message_id).unwrap_or(&0);
                                    ss.3.insert(message_id.clone(), n + 1);
                                    st.remember_message(format!("{message_id}:{n}"), txt);
                                    st.refresh_output();
                                } else if btype == "thinking"
                                    && let Some(thinking) =
                                        block.get("thinking").and_then(Value::as_str)
                                {
                                    let trimmed = thinking.trim();
                                    if !trimmed.is_empty() {
                                        let id = format!("thinking-{message_id}-{}", st.steps.len());
                                        let label = st.limit(trimmed, 2000);
                                        st.record_step(AgentStep {
                                            id,
                                            kind: "reasoning".into(),
                                            label,
                                            status: "completed".into(),
                                            exit_code: None,
                                            output: None,
                                            items: None,
                                        });
                                    }
                                } else if btype == "tool_use"
                                    && let Some(raw_id) = block.get("id").and_then(Value::as_str)
                                {
                                    let name = st.text(block.get("name"), 100);
                                    let input_obj = block
                                        .get("input")
                                        .and_then(Value::as_object)
                                        .cloned()
                                        .unwrap_or_default();
                                    let step_id = st.limit(raw_id, 200);
                                    if name == "Bash" {
                                        let cmd_label = st.text(input_obj.get("command"), 2000);
                                        let cmd_act = st.text(input_obj.get("command"), 500);
                                        st.record_step(AgentStep {
                                            id: step_id,
                                            kind: "command".into(),
                                            label: if cmd_label.is_empty() {
                                                "Command".into()
                                            } else {
                                                cmd_label
                                            },
                                            status: "running".into(),
                                            exit_code: None,
                                            output: None,
                                            items: None,
                                        });
                                        let act = if cmd_act.is_empty() {
                                            "Running command".into()
                                        } else {
                                            st.limit(&cmd_act, 500)
                                        };
                                        st.push_activity(act);
                                    } else if matches!(
                                        name.as_str(),
                                        "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
                                    ) {
                                        let path_val = input_obj
                                            .get("file_path")
                                            .or_else(|| input_obj.get("notebook_path"));
                                        let rel = self.relative(&mut st, path_val);
                                        st.record_step(AgentStep {
                                            id: step_id,
                                            kind: "edit".into(),
                                            label: if rel.is_empty() {
                                                "Workspace files".into()
                                            } else {
                                                rel
                                            },
                                            status: "running".into(),
                                            exit_code: None,
                                            output: None,
                                            items: None,
                                        });
                                        st.push_activity("Editing workspace files".into());
                                    } else if name == "TodoWrite"
                                        && let Some(todos) =
                                            input_obj.get("todos").and_then(Value::as_array)
                                    {
                                        let items: Vec<StepItem> = todos
                                            .iter()
                                            .take(50)
                                            .map(|entry| {
                                                let o = entry.as_object();
                                                let txt = st.text(o.and_then(|x| x.get("content")), 500);
                                                let done = o
                                                    .and_then(|x| x.get("status"))
                                                    .and_then(Value::as_str)
                                                    == Some("completed");
                                                StepItem {
                                                    text: if txt.is_empty() {
                                                        "Step".into()
                                                    } else {
                                                        txt
                                                    },
                                                    done,
                                                }
                                            })
                                            .collect();
                                        st.record_step(AgentStep {
                                            id: "plan-todos".into(),
                                            kind: "plan".into(),
                                            label: "Plan".into(),
                                            status: "running".into(),
                                            exit_code: None,
                                            output: None,
                                            items: Some(items),
                                        });
                                    } else if name == "Grep" || name == "Glob" {
                                        let pat = st.text(input_obj.get("pattern"), 1000);
                                        let label = st.limit(&format!("{name} {pat}"), 2000);
                                        st.record_step(AgentStep {
                                            id: step_id,
                                            kind: "search".into(),
                                            label,
                                            status: "running".into(),
                                            exit_code: None,
                                            output: None,
                                            items: None,
                                        });
                                    } else if name == "Read" {
                                        let rel = self.relative(&mut st, input_obj.get("file_path"));
                                        let label = st.limit(&format!("Read {rel}"), 2000);
                                        st.record_step(AgentStep {
                                            id: step_id,
                                            kind: "tool".into(),
                                            label,
                                            status: "running".into(),
                                            exit_code: None,
                                            output: None,
                                            items: None,
                                        });
                                    } else if name != "AskUserQuestion" && name != "ExitPlanMode" {
                                        st.record_step(AgentStep {
                                            id: step_id,
                                            kind: "tool".into(),
                                            label: if name.is_empty() {
                                                "Tool call".into()
                                            } else {
                                                name
                                            },
                                            status: "running".into(),
                                            exit_code: None,
                                            output: None,
                                            items: None,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    "user" => {
                        let content = m
                            .get("message")
                            .and_then(Value::as_object)
                            .and_then(|msg| msg.get("content"))
                            .and_then(Value::as_array);
                        let Some(blocks) = content else { return };
                        let mut st = self.state.lock().expect("state");
                        static RE_EXIT: OnceLock<Regex> = OnceLock::new();
                        let re_exit =
                            RE_EXIT.get_or_init(|| Regex::new(r"^Exit code (\d+)").expect("regex"));
                        for block_val in blocks {
                            let Some(block) = block_val.as_object() else {
                                continue;
                            };
                            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                                continue;
                            }
                            let Some(tool_use_id) =
                                block.get("tool_use_id").and_then(Value::as_str)
                            else {
                                continue;
                            };
                            let Some(mut step) = st.steps.get(tool_use_id).cloned() else {
                                continue;
                            };
                            let body = match block.get("content") {
                                Some(Value::String(s)) => s.clone(),
                                Some(Value::Array(parts)) => parts
                                    .iter()
                                    .map(|part| {
                                        part.as_object()
                                            .filter(|o| {
                                                o.get("type").and_then(Value::as_str) == Some("text")
                                            })
                                            .and_then(|o| o.get("text"))
                                            .and_then(Value::as_str)
                                            .unwrap_or("")
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => String::new(),
                            };
                            let failed = block.get("is_error") == Some(&Value::Bool(true));
                            step.status = if failed { "failed" } else { "completed" }.into();
                            if step.kind == "command" {
                                let code = re_exit
                                    .captures(&body)
                                    .and_then(|c| c.get(1))
                                    .and_then(|m| m.as_str().parse::<i64>().ok())
                                    .unwrap_or(if failed { 1 } else { 0 });
                                step.exit_code = Some(code);
                                step.output = Some(tail_utf16(&body, 2000).to_owned());
                                st.remember_output(step.id.clone(), &body);
                                st.record_step(step);
                                st.refresh_output();
                            } else {
                                st.record_step(step);
                            }
                        }
                    }
                    "result" => {
                        let mut st = self.state.lock().expect("state");
                        if let Some(usage) = m.get("usage").and_then(Value::as_object) {
                            let input = count_u64(usage.get("input_tokens"))
                                + count_u64(usage.get("cache_read_input_tokens"))
                                + count_u64(usage.get("cache_creation_input_tokens"));
                            let cached = count_u64(usage.get("cache_read_input_tokens"));
                            let output = count_u64(usage.get("output_tokens"));
                            st.data.usage = Some(AgentUsage {
                                input,
                                cached,
                                output,
                            });
                        }
                        if let Some(model_usage) = m.get("modelUsage").and_then(Value::as_object) {
                            for val in model_usage.values() {
                                if let Some(cw) = val
                                    .as_object()
                                    .and_then(|o| o.get("contextWindow"))
                                    .and_then(|v| safe_i64(Some(v)))
                                    .filter(|&w| w >= 0)
                                {
                                    st.data.context_window = Some(cw as u64);
                                    break;
                                }
                            }
                        }
                        let st_val = if m.get("subtype").and_then(Value::as_str) == Some("success")
                            && m.get("is_error") != Some(&Value::Bool(true))
                        {
                            "completed"
                        } else {
                            "failed"
                        };
                        if st_val == "failed" {
                            let detail = if let Some(s) = m.get("result").and_then(Value::as_str) {
                                s.to_owned()
                            } else if let Some(errs) = m.get("errors").and_then(Value::as_array) {
                                errs.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            } else {
                                String::new()
                            };
                            if !detail.is_empty() {
                                st.data.error = st.limit(&detail, 2000);
                            }
                        }
                        st.dirty = true;
                        drop(st);
                        *self.status.lock().expect("status") = Some(st_val);
                        if let Some(proc) = self.proc_slot.lock().expect("proc").as_ref() {
                            proc.end();
                        }
                    }
                    _ => {}
                }
            }

            fn request(&self, subtype: String, request: Json) -> ControlRequestFuture {
                let state = self.state.clone();
                let entry = self.entry.clone();
                let cancel = self.cancel.clone();
                let ws_prefix = self.workspace_prefix.clone();
                Box::pin(async move {
                    if subtype != "can_use_tool" {
                        return Err(AppError::with(
                            "declined",
                            403,
                            "Dar does not support this request.",
                        ));
                    }
                    let (tool, input_obj) = {
                        let mut st = state.lock().expect("state");
                        let tool = st.text(request.get("tool_name"), 100);
                        let input_obj = request
                            .get("input")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        (tool, input_obj)
                    };
                    if tool == "AskUserQuestion"
                        && let Some(raw_arr) = input_obj.get("questions").and_then(Value::as_array)
                    {
                        let raw: Vec<Map<String, Value>> = raw_arr
                            .iter()
                            .take(10)
                            .map(|v| v.as_object().cloned().unwrap_or_default())
                            .collect();
                        let questions: Vec<AgentQuestion> = {
                            let mut st = state.lock().expect("state");
                            raw.iter()
                                .enumerate()
                                .map(|(index, q)| {
                                    let options = q
                                        .get("options")
                                        .and_then(Value::as_array)
                                        .map(|arr| {
                                            arr.iter()
                                                .take(20)
                                                .map(|opt| {
                                                    let o = opt.as_object();
                                                    QuestionOption {
                                                        label: st.text(
                                                            o.and_then(|x| x.get("label")),
                                                            200,
                                                        ),
                                                        description: st.text(
                                                            o.and_then(|x| x.get("description")),
                                                            1000,
                                                        ),
                                                    }
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    AgentQuestion {
                                        id: index.to_string(),
                                        header: st.text(q.get("header"), 200),
                                        question: st.text(q.get("question"), 2000),
                                        options,
                                        other: true,
                                        secret: false,
                                    }
                                })
                                .collect()
                        };
                        let answers =
                            AgentRuntime::ask_user(entry, state, cancel, questions).await?;
                        let mut answer_map = Map::new();
                        for (index, q) in raw.iter().enumerate() {
                            let q_text = q
                                .get("question")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned();
                            let joined = answers
                                .get(&index.to_string())
                                .map(|v| v.join(", "))
                                .unwrap_or_default();
                            answer_map.insert(q_text, json!(joined));
                        }
                        let mut updated_input = input_obj;
                        updated_input.insert("answers".into(), Value::Object(answer_map));
                        let mut resp = Map::new();
                        resp.insert("behavior".into(), json!("allow"));
                        resp.insert("updatedInput".into(), Value::Object(updated_input));
                        return Ok(resp);
                    }
                    if tool == "ExitPlanMode" {
                        let mut st = state.lock().expect("state");
                        let plan = st.text(input_obj.get("plan"), 60_000);
                        if !plan.is_empty() {
                            st.remember_message(format!("plan-{}", uuid()), &plan);
                            st.refresh_output();
                        }
                        let mut resp = Map::new();
                        resp.insert("behavior".into(), json!("deny"));
                        resp.insert(
                            "message".into(),
                            json!("Dar saved this plan for the user to review. Stop now without making changes."),
                        );
                        return Ok(resp);
                    }
                    {
                        let mut st = state.lock().expect("state");
                        let detail = if tool == "SandboxNetworkAccess" {
                            format!("network access to {}", st.text(input_obj.get("host"), 200))
                        } else {
                            let cmd = st.text(input_obj.get("command"), 1500);
                            if !cmd.is_empty() {
                                cmd
                            } else {
                                let fp = st.text(input_obj.get("file_path"), 2000);
                                let rel = fp
                                    .strip_prefix(&ws_prefix)
                                    .map(str::to_owned)
                                    .unwrap_or(fp);
                                if !rel.is_empty() {
                                    rel
                                } else if !tool.is_empty() {
                                    tool
                                } else {
                                    "request outside Dar's sandbox".into()
                                }
                            }
                        };
                        let label = st.limit(&format!("Declined: {detail}"), 2000);
                        st.record_step(AgentStep {
                            id: format!("declined-{}", uuid()),
                            kind: "tool".into(),
                            label,
                            status: "failed".into(),
                            exit_code: None,
                            output: None,
                            items: None,
                        });
                    }
                    let mut resp = Map::new();
                    resp.insert("behavior".into(), json!("deny"));
                    resp.insert(
                        "message".into(),
                        json!("Dar declines requests outside its sandbox."),
                    );
                    Ok(resp)
                })
            }
        }

        let attempt = |resume: Option<String>, prompt: String| {
            let claude_config = claude_config.clone();
            let workspace = prepared.workspace.clone();
            let model = model.clone();
            let effort = effort.clone();
            let plan = matches!(mode.as_deref(), Some("plan" | "review"));
            let deny = deny.clone();
            let images = images.clone();
            let state = state.clone();
            let entry = entry.clone();
            let cancel = cancel.clone();
            let ws_prefix = workspace_prefix.clone();
            let thread_rev = history.thread.revision;
            async move {
                let initialized = Arc::new(AtomicBool::new(false));
                let status: Arc<Mutex<Option<&'static str>>> = Arc::new(Mutex::new(None));
                let proc_slot: Arc<Mutex<Option<ClaudeProcess>>> = Arc::new(Mutex::new(None));
                let args = claude_args(&ClaudeRunOptions {
                    workspace: &workspace,
                    model: &model,
                    effort: effort.as_deref(),
                    plan,
                    resume: resume.as_deref(),
                    commands,
                    deny: &deny,
                })?;
                let handlers = TurnHandlers {
                    state: state.clone(),
                    entry: entry.clone(),
                    cancel: cancel.clone(),
                    thread_revision: thread_rev,
                    workspace_prefix: ws_prefix,
                    initialized: initialized.clone(),
                    status: status.clone(),
                    proc_slot: proc_slot.clone(),
                    stream_state: Mutex::new((
                        String::new(),
                        String::new(),
                        HashMap::new(),
                        HashMap::new(),
                    )),
                };
                let claude = open_claude(&claude_config, &workspace, &args, handlers);
                *proc_slot.lock().expect("proc") = Some(claude.clone());

                let claude_abort = claude.clone();
                let cancel_watch = cancel.clone();
                let abort_task = tokio::spawn(async move {
                    cancel_watch.cancelled().await;
                    let c2 = claude_abort.clone();
                    tokio::spawn(async move {
                        let _ = c2.control("interrupt", Map::new()).await;
                    });
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    claude_abort.close(None);
                });

                {
                    let st_steer = state.clone();
                    *entry.steer_fn.lock().expect("steer_fn") = Some(Arc::new(move |msg_id| {
                        let mut st = st_steer.lock().expect("state");
                        let list = st.data.steered.get_or_insert_with(Vec::new);
                        list.push(msg_id);
                        if list.len() > 50 {
                            let drop_n = list.len() - 50;
                            list.drain(..drop_n);
                        }
                        st.dirty = true;
                    }));
                    let c_deliver = claude.clone();
                    let status_deliver = status.clone();
                    *entry.deliver_fn.lock().expect("deliver_fn") =
                        Some(Arc::new(move |content: String| {
                            let c = c_deliver.clone();
                            let s = status_deliver.clone();
                            Box::pin(async move {
                                if s.lock().expect("status").is_some() {
                                    return Err(AppError::with(
                                        "turnUnavailable",
                                        409,
                                        "This turn is not running. Send the message as a new turn.",
                                    ));
                                }
                                c.send(&json!({
                                    "type": "user",
                                    "message": { "role": "user", "content": [{ "type": "text", "text": content }] },
                                    "parent_tool_use_id": null,
                                    "session_id": "",
                                }));
                                Ok(())
                            })
                        }));
                }

                let mut blocks = vec![json!({ "type": "text", "text": prompt })];
                blocks.extend(images);
                claude.send(&json!({
                    "type": "user",
                    "message": { "role": "user", "content": blocks },
                    "parent_tool_use_id": null,
                    "session_id": "",
                }));
                let _ = claude.exited().await;

                abort_task.abort();
                claude.close(None);
                *entry.deliver_fn.lock().expect("deliver_fn") = None;

                Ok::<AttemptOutcome, AppError>(AttemptOutcome {
                    initialized: initialized.load(Ordering::SeqCst),
                    status: *status.lock().expect("status"),
                    diagnostics: claude.diagnostics(),
                })
            }
        };

        let prev_session = previous.and_then(|p| p.claude_session.clone());
        let mut outcome = attempt(prev_session.clone(), input.clone()).await?;
        if !outcome.initialized && prev_session.is_some() && !cancel.is_cancelled() {
            if mode.as_deref() != Some("review") {
                input = format_messages(&history.items);
            }
            outcome = attempt(None, input).await?;
        }
        cancel.throw_if_cancelled()?;
        if let Some(st) = outcome.status {
            return Ok(st);
        }
        if outcome
            .diagnostics
            .to_ascii_lowercase()
            .contains("refusing to start without a working sandbox")
        {
            return Err(AppError::with(
                "sandboxUnavailable",
                503,
                "Claude Code could not establish its command sandbox (bubblewrap, socat and unprivileged user namespaces are required). No model run was started.",
            ));
        }
        Err(AppError::with(
            "claudeIncomplete",
            502,
            "Claude Code exited before completing the turn.",
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_codex_turn(
        &self,
        config: &CodexConfig,
        row: &ClaimedRunRow,
        history: &AgentHistoryRow,
        unseen: &[AgentMessageRow],
        previous: Option<&AgentRunDocument>,
        prepared: &crate::agent::workspace::PreparedWorkspace,
        attachments: &[(AgentAttachment, PathBuf)],
        state: Arc<Mutex<RunDocState>>,
        entry: Arc<ActiveRun>,
        cancel: CancelToken,
    ) -> Result<&'static str> {
        let probe = self.root.join("agent-workspaces").join("isolation-probe");
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&probe)
            .await
        {
            Ok(mut f) => {
                f.write_all(b"Dar isolation check\n")
                    .await
                    .map_err(|_| AppError::internal())?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta = tokio::fs::symlink_metadata(&probe)
                    .await
                    .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
                if !meta.is_file() || meta.file_type().is_symlink() {
                    return Err(AppError::new("unsafeWorkspace", 409));
                }
            }
            Err(_) => return Err(AppError::internal()),
        }

        if !codex_account(config, &prepared.workspace, Some(cancel.clone())).await? {
            return Err(AppError::with(
                "codexLoginRequired",
                409,
                "The configured Codex CLI ChatGPT login is no longer available.",
            ));
        }
        verify_codex_sandbox(config, &prepared.workspace, &probe, Some(cancel.clone())).await?;
        assert_no_codex_rules(config).await?;

        let (comp_tx, comp_rx) = oneshot::channel::<Result<&'static str>>();
        let completion = Arc::new(Mutex::new(Some(comp_tx)));

        struct CodexTurnHandlers {
            state: Arc<Mutex<RunDocState>>,
            entry: Arc<ActiveRun>,
            cancel: CancelToken,
            completion: Arc<Mutex<Option<oneshot::Sender<Result<&'static str>>>>>,
        }

        fn turn_status(item: &Map<String, Value>, done: bool) -> &'static str {
            if !done {
                return "running";
            }
            match item.get("status").and_then(Value::as_str) {
                Some("failed" | "declined") => "failed",
                _ => "completed",
            }
        }

        impl AppServerHandlers for CodexTurnHandlers {
            fn notify(&self, method: &str, params: Json) {
                let done = method == "item/completed";
                if (method == "item/started" || done)
                    && let Some(item) = params.get("item").and_then(Value::as_object)
                {
                    let mut st = self.state.lock().expect("state");
                    let raw_id = st.text(item.get("id"), 200);
                    let item_id = if raw_id.is_empty() { uuid() } else { raw_id };
                    let prev_step = st.steps.get(&item_id).cloned();
                    let itype = item.get("type").and_then(Value::as_str).unwrap_or("");
                    match itype {
                        "commandExecution" => {
                            let exit = safe_i64(item.get("exitCode"));
                            let tail = item
                                .get("aggregatedOutput")
                                .and_then(Value::as_str)
                                .map(|s| tail_utf16(s, 2000).to_owned())
                                .or_else(|| prev_step.as_ref().and_then(|p| p.output.clone()));
                            let cmd_text = st.text(item.get("command"), 2000);
                            let label = if !cmd_text.is_empty() {
                                cmd_text
                            } else {
                                prev_step
                                    .as_ref()
                                    .map(|p| p.label.clone())
                                    .unwrap_or_else(|| "Command".into())
                            };
                            let status = if done {
                                if matches!(
                                    item.get("status").and_then(Value::as_str),
                                    Some("failed" | "declined")
                                ) || exit.is_some_and(|c| c != 0)
                                {
                                    "failed"
                                } else {
                                    "completed"
                                }
                            } else {
                                "running"
                            };
                            st.record_step(AgentStep {
                                id: item_id.clone(),
                                kind: "command".into(),
                                label,
                                status: status.into(),
                                exit_code: exit,
                                output: tail,
                                items: None,
                            });
                            if let Some(agg) =
                                item.get("aggregatedOutput").and_then(Value::as_str)
                            {
                                st.remember_output(item_id, agg);
                                st.refresh_output();
                            }
                            if !done {
                                let act_cmd = st.text(item.get("command"), 500);
                                let act = if act_cmd.is_empty() {
                                    "Running command".into()
                                } else {
                                    st.limit(&act_cmd, 500)
                                };
                                st.push_activity(act);
                            }
                        }
                        "fileChange" => {
                            let paths: Vec<String> = item
                                .get("changes")
                                .and_then(Value::as_array)
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|c| {
                                            c.as_object()
                                                .and_then(|o| o.get("path"))
                                                .and_then(Value::as_str)
                                                .filter(|s| !s.is_empty())
                                                .map(str::to_owned)
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            let joined = paths.join("\n");
                            let raw_label = if !joined.is_empty() {
                                joined
                            } else {
                                prev_step
                                    .as_ref()
                                    .map(|p| p.label.clone())
                                    .unwrap_or_else(|| "Workspace files".into())
                            };
                            let label = st.limit(&raw_label, 2000);
                            st.record_step(AgentStep {
                                id: item_id,
                                kind: "edit".into(),
                                label,
                                status: turn_status(item, done).into(),
                                exit_code: None,
                                output: None,
                                items: None,
                            });
                            if !done {
                                st.push_activity("Editing workspace files".into());
                            }
                        }
                        "reasoning" => {
                            let summary_arr = item.get("summary").and_then(Value::as_array);
                            let content_arr = item.get("content").and_then(Value::as_array);
                            let mut parts: Vec<&str> = Vec::new();
                            if let Some(s) = summary_arr {
                                for v in s {
                                    if let Some(t) = v.as_str() {
                                        parts.push(t);
                                    }
                                }
                            }
                            if summary_arr.is_none_or(|s| s.is_empty())
                                && let Some(c) = content_arr
                            {
                                for v in c {
                                    if let Some(t) = v.as_str() {
                                        parts.push(t);
                                    }
                                }
                            }
                            let summary = parts.join("\n").trim().to_owned();
                            if !summary.is_empty() {
                                let label = st.limit(&summary, 2000);
                                st.record_step(AgentStep {
                                    id: item_id,
                                    kind: "reasoning".into(),
                                    label,
                                    status: if done { "completed" } else { "running" }.into(),
                                    exit_code: None,
                                    output: None,
                                    items: None,
                                });
                            }
                        }
                        "agentMessage" => {
                            if let Some(txt) = item.get("text").and_then(Value::as_str) {
                                st.remember_message(item_id, txt);
                                st.refresh_output();
                            }
                        }
                        "mcpToolCall" => {
                            let server_name = st.text(item.get("server"), 200);
                            let tool_name = st.text(item.get("tool"), 200);
                            let tool_label = if tool_name.is_empty() {
                                "Tool call"
                            } else {
                                &tool_name
                            };
                            let combined = if server_name.is_empty() {
                                tool_label.to_owned()
                            } else {
                                format!("{server_name} \u{00b7} {tool_label}")
                            };
                            let label = st.limit(&combined, 2000);
                            st.record_step(AgentStep {
                                id: item_id,
                                kind: "tool".into(),
                                label,
                                status: turn_status(item, done).into(),
                                exit_code: None,
                                output: None,
                                items: None,
                            });
                        }
                        "webSearch" => {
                            let q = st.text(item.get("query"), 2000);
                            st.record_step(AgentStep {
                                id: item_id,
                                kind: "search".into(),
                                label: if q.is_empty() { "Web search".into() } else { q },
                                status: if done { "completed" } else { "running" }.into(),
                                exit_code: None,
                                output: None,
                                items: None,
                            });
                        }
                        _ => {}
                    }
                    return;
                }

                match method {
                    "item/agentMessage/delta" => {
                        if let (Some(item_id), Some(delta)) = (
                            params.get("itemId").and_then(Value::as_str),
                            params.get("delta").and_then(Value::as_str),
                        ) {
                            let mut st = self.state.lock().expect("state");
                            let prev = st.messages.get(item_id).cloned().unwrap_or_default();
                            let next = format!("{prev}{delta}");
                            st.remember_message(item_id.to_owned(), &next);
                            st.refresh_output();
                        }
                    }
                    "item/commandExecution/outputDelta" => {
                        if let (Some(item_id), Some(delta)) = (
                            params.get("itemId").and_then(Value::as_str),
                            params.get("delta").and_then(Value::as_str),
                        ) {
                            let mut st = self.state.lock().expect("state");
                            let prev = st.outputs.get(item_id).cloned().unwrap_or_default();
                            let all = format!("{prev}{delta}");
                            st.remember_output(item_id.to_owned(), &all);
                            if let Some(mut step) = st.steps.get(item_id).cloned() {
                                step.output = Some(tail_utf16(&all, 2000).to_owned());
                                st.record_step(step);
                            }
                            st.refresh_output();
                        }
                    }
                    "turn/plan/updated" => {
                        if let Some(plan) = params.get("plan").and_then(Value::as_array) {
                            let mut st = self.state.lock().expect("state");
                            let tid = st.text(params.get("turnId"), 100);
                            let expl = st.text(params.get("explanation"), 2000);
                            let items: Vec<StepItem> = plan
                                .iter()
                                .take(50)
                                .map(|entry| {
                                    let o = entry.as_object();
                                    let txt = st.text(o.and_then(|x| x.get("step")), 500);
                                    let done = o
                                        .and_then(|x| x.get("status"))
                                        .and_then(Value::as_str)
                                        == Some("completed");
                                    StepItem {
                                        text: if txt.is_empty() { "Step".into() } else { txt },
                                        done,
                                    }
                                })
                                .collect();
                            st.record_step(AgentStep {
                                id: format!("plan-{tid}"),
                                kind: "plan".into(),
                                label: if expl.is_empty() { "Plan".into() } else { expl },
                                status: "running".into(),
                                exit_code: None,
                                output: None,
                                items: Some(items),
                            });
                        }
                    }
                    "thread/tokenUsage/updated" => {
                        if let Some(usage) = params.get("tokenUsage").and_then(Value::as_object) {
                            let last = usage.get("last").and_then(Value::as_object);
                            let total = usage.get("total").and_then(Value::as_object);
                            let mut st = self.state.lock().expect("state");
                            st.data.usage = Some(AgentUsage {
                                input: count_u64(total.and_then(|t| t.get("inputTokens"))),
                                cached: count_u64(total.and_then(|t| t.get("cachedInputTokens"))),
                                output: count_u64(total.and_then(|t| t.get("outputTokens"))),
                            });
                            let last_in = count_u64(last.and_then(|l| l.get("inputTokens")));
                            if last_in > 0 {
                                st.data.context_used = Some(
                                    last_in + count_u64(last.and_then(|l| l.get("outputTokens"))),
                                );
                            }
                            if let Some(cw) =
                                safe_i64(usage.get("modelContextWindow")).filter(|&w| w >= 0)
                            {
                                st.data.context_window = Some(cw as u64);
                            }
                            st.dirty = true;
                        }
                    }
                    "thread/compacted" => {
                        let is_compact = {
                            let mut st = self.state.lock().expect("state");
                            st.record_step(AgentStep {
                                id: "compacted".into(),
                                kind: "tool".into(),
                                label: "Compacted the conversation context".into(),
                                status: "completed".into(),
                                exit_code: None,
                                output: None,
                                items: None,
                            });
                            st.data.mode.as_deref() == Some("compact")
                        };
                        if is_compact
                            && let Some(tx) = self.completion.lock().expect("comp").take()
                        {
                            let _ = tx.send(Ok("completed"));
                        }
                    }
                    "turn/started" => {
                        let non_default = self
                            .state
                            .lock()
                            .expect("state")
                            .data
                            .mode
                            .as_deref()
                            .is_some_and(|m| m != "default");
                        if non_default
                            && let Some(tid) = params
                                .get("turn")
                                .and_then(Value::as_object)
                                .and_then(|t| t.get("id"))
                                .and_then(Value::as_str)
                        {
                            *self.entry.turn.lock().expect("turn") = Some(tid.to_owned());
                        }
                    }
                    "turn/completed" => {
                        let turn = params.get("turn").and_then(Value::as_object);
                        if let Some(msg) = turn
                            .and_then(|t| t.get("error"))
                            .and_then(Value::as_object)
                            .and_then(|e| e.get("message"))
                            .and_then(Value::as_str)
                        {
                            let mut st = self.state.lock().expect("state");
                            st.data.error = st.limit(msg, 2000);
                        }
                        let st_val = match turn.and_then(|t| t.get("status")).and_then(Value::as_str)
                        {
                            Some("completed") => "completed",
                            Some("interrupted") => "cancelled",
                            _ => "failed",
                        };
                        if let Some(tx) = self.completion.lock().expect("comp").take() {
                            let _ = tx.send(Ok(st_val));
                        }
                    }
                    "error" => {
                        if params.get("willRetry") != Some(&Value::Bool(true)) {
                            let mut st = self.state.lock().expect("state");
                            if let Some(msg) = params
                                .get("error")
                                .and_then(Value::as_object)
                                .and_then(|e| e.get("message"))
                                .and_then(Value::as_str)
                            {
                                st.data.error = st.limit(msg, 2000);
                            }
                            st.dirty = true;
                        }
                    }
                    _ => {}
                }
            }

            fn request(&self, method: String, params: Json) -> RequestFuture {
                let state = self.state.clone();
                let entry = self.entry.clone();
                let cancel = self.cancel.clone();
                Box::pin(async move {
                    if method == "item/tool/requestUserInput" {
                        let questions: Vec<AgentQuestion> = {
                            let mut st = state.lock().expect("state");
                            params
                                .get("questions")
                                .and_then(Value::as_array)
                                .map(|arr| {
                                    arr.iter()
                                        .take(10)
                                        .map(|input| {
                                            let q = input.as_object();
                                            let qid = st.text(q.and_then(|x| x.get("id")), 200);
                                            let options = q
                                                .and_then(|x| x.get("options"))
                                                .and_then(Value::as_array)
                                                .map(|opts| {
                                                    opts.iter()
                                                        .take(20)
                                                        .map(|opt| {
                                                            let o = opt.as_object();
                                                            QuestionOption {
                                                                label: st.text(
                                                                    o.and_then(|x| x.get("label")),
                                                                    200,
                                                                ),
                                                                description: st.text(
                                                                    o.and_then(|x| {
                                                                        x.get("description")
                                                                    }),
                                                                    1000,
                                                                ),
                                                            }
                                                        })
                                                        .collect()
                                                })
                                                .unwrap_or_default();
                                            AgentQuestion {
                                                id: if qid.is_empty() { uuid() } else { qid },
                                                header: st
                                                    .text(q.and_then(|x| x.get("header")), 200),
                                                question: st
                                                    .text(q.and_then(|x| x.get("question")), 2000),
                                                options,
                                                other: q.and_then(|x| x.get("isOther"))
                                                    == Some(&Value::Bool(true)),
                                                secret: q.and_then(|x| x.get("isSecret"))
                                                    == Some(&Value::Bool(true)),
                                            }
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        };
                        let answers =
                            AgentRuntime::ask_user(entry, state, cancel, questions).await?;
                        let mut mapped = Map::new();
                        for (k, v) in answers {
                            mapped.insert(k, json!({ "answers": v }));
                        }
                        return Ok(json!({ "answers": Value::Object(mapped) }));
                    }
                    if matches!(
                        method.as_str(),
                        "item/commandExecution/requestApproval"
                            | "item/fileChange/requestApproval"
                            | "execCommandApproval"
                            | "applyPatchApproval"
                    ) {
                        {
                            let mut st = state.lock().expect("state");
                            let cmd = st.text(params.get("command"), 1500);
                            let reason = st.text(params.get("reason"), 1500);
                            let detail = if !cmd.is_empty() {
                                cmd
                            } else if !reason.is_empty() {
                                reason
                            } else {
                                "request outside Dar's sandbox".into()
                            };
                            let label = st.limit(&format!("Declined: {detail}"), 2000);
                            st.record_step(AgentStep {
                                id: format!("declined-{}", uuid()),
                                kind: "tool".into(),
                                label,
                                status: "failed".into(),
                                exit_code: None,
                                output: None,
                                items: None,
                            });
                        }
                        let decision = if method.starts_with("item/") {
                            "decline"
                        } else {
                            "denied"
                        };
                        return Ok(json!({ "decision": decision }));
                    }
                    Err(AppError::with(
                        "declined",
                        403,
                        "Dar does not support this request.",
                    ))
                })
            }
        }

        let server = open_app_server(
            config,
            &prepared.workspace,
            CodexTurnHandlers {
                state: state.clone(),
                entry: entry.clone(),
                cancel: cancel.clone(),
                completion: completion.clone(),
            },
            &["-c", "approval_policy=\"never\""],
        );

        let srv_abort = server.clone();
        let cancel_watch = cancel.clone();
        let entry_abort = entry.clone();
        let state_abort = state.clone();
        let abort_task = tokio::spawn(async move {
            cancel_watch.cancelled().await;
            let turn_id = entry_abort.turn.lock().expect("turn").clone();
            let thread_id = state_abort.lock().expect("state").data.codex_thread.clone();
            if let (Some(turn_id), Some(thread_id)) = (turn_id, thread_id) {
                let s2 = srv_abort.clone();
                tokio::spawn(async move {
                    let _ = s2
                        .request(
                            "turn/interrupt",
                            json!({ "threadId": thread_id, "turnId": turn_id }),
                        )
                        .await;
                });
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
            srv_abort.close(None);
        });

        let srv_exit = server.clone();
        let comp_exit = completion.clone();
        let exit_task = tokio::spawn(async move {
            let _ = srv_exit.exited().await;
            if let Some(tx) = comp_exit.lock().expect("comp").take() {
                let _ = tx.send(Err(AppError::with(
                    "codexIncomplete",
                    502,
                    "The Codex CLI exited before reporting a completed turn.",
                )));
            }
        });

        let turn_res: Result<&'static str> = async {
            server
                .request(
                    "initialize",
                    json!({
                        "clientInfo": { "name": "dar", "title": "Dar", "version": "1.0.0" },
                        "capabilities": { "experimentalApi": true },
                    }),
                )
                .await?;
            server.notify("initialized", json!({}));

            let ws_str = prepared.workspace.to_string_lossy().into_owned();
            let mut thread_key: Option<String> = None;
            let mut input = format_messages(&history.items);
            if let Some(prev_thread) = previous.and_then(|p| p.codex_thread.as_ref())
                && let Ok(resumed) = server
                    .request(
                        "thread/resume",
                        json!({
                            "threadId": prev_thread,
                            "cwd": ws_str,
                            "approvalPolicy": "never",
                            "developerInstructions": CODEX_INSTRUCTIONS,
                        }),
                    )
                    .await
                && let Some(tid) = resumed
                    .get("thread")
                    .and_then(|t| t.get("id"))
                    .and_then(Value::as_str)
            {
                thread_key = Some(tid.to_owned());
                input = format_messages(unseen);
            }
            let thread_key = match thread_key {
                Some(k) => k,
                None => {
                    let started = server
                        .request(
                            "thread/start",
                            json!({
                                "model": row.model,
                                "cwd": ws_str,
                                "approvalPolicy": "never",
                                "developerInstructions": CODEX_INSTRUCTIONS,
                            }),
                        )
                        .await?;
                    started
                        .get("thread")
                        .and_then(|t| t.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned()
                }
            };
            if thread_key.is_empty()
                || thread_key.len() > 100
                || !thread_key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err(AppError::new("invalidProviderResponse", 502));
            }
            let (effort, mode) = {
                let mut st = state.lock().expect("state");
                st.data.codex_thread = Some(thread_key.clone());
                st.data.context_revision = Some(history.thread.revision);
                st.dirty = true;
                (st.data.effort.clone(), st.data.mode.clone())
            };
            if mode.as_deref() == Some("compact")
                && previous.and_then(|p| p.codex_thread.as_ref()).is_none()
            {
                return Err(AppError::with(
                    "nothingToCompact",
                    409,
                    "There is no Codex context to compact yet. Run a turn first.",
                ));
            }

            let turn_id = if mode.as_deref() == Some("review") {
                let res = server
                    .request(
                        "review/start",
                        json!({
                            "threadId": thread_key,
                            "target": { "type": "uncommittedChanges" },
                        }),
                    )
                    .await?;
                res.get("turn")
                    .and_then(|t| t.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            } else if mode.as_deref() == Some("compact") {
                server
                    .request("thread/compact/start", json!({ "threadId": thread_key }))
                    .await?;
                "compact".to_owned()
            } else {
                let mut input_items = vec![json!({
                    "type": "text",
                    "text": input,
                    "text_elements": [],
                })];
                for (_, path) in attachments {
                    input_items.push(json!({
                        "type": "localImage",
                        "path": path.to_string_lossy(),
                    }));
                }
                let mut params = Map::new();
                params.insert("threadId".into(), json!(thread_key));
                params.insert("input".into(), Value::Array(input_items));
                params.insert("model".into(), json!(row.model));
                params.insert("approvalPolicy".into(), json!("never"));
                if let Some(ref eff) = effort {
                    params.insert("effort".into(), json!(eff));
                }
                if mode.as_deref() == Some("plan") {
                    params.insert(
                        "collaborationMode".into(),
                        json!({
                            "mode": "plan",
                            "settings": {
                                "model": row.model,
                                "reasoning_effort": effort,
                                "developer_instructions": null,
                            }
                        }),
                    );
                }
                let res = server.request("turn/start", Value::Object(params)).await?;
                res.get("turn")
                    .and_then(|t| t.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };

            {
                *entry.turn.lock().expect("turn") = Some(turn_id.clone());
                let st_steer = state.clone();
                *entry.steer_fn.lock().expect("steer_fn") = Some(Arc::new(move |msg_id| {
                    let mut st = st_steer.lock().expect("state");
                    let list = st.data.steered.get_or_insert_with(Vec::new);
                    list.push(msg_id);
                    if list.len() > 50 {
                        let drop_n = list.len() - 50;
                        list.drain(..drop_n);
                    }
                    st.dirty = true;
                }));
                let srv_deliver = server.clone();
                let tk_deliver = thread_key.clone();
                let tid_deliver = turn_id.clone();
                *entry.deliver_fn.lock().expect("deliver_fn") =
                    Some(Arc::new(move |content: String| {
                        let srv = srv_deliver.clone();
                        let tk = tk_deliver.clone();
                        let tid = tid_deliver.clone();
                        Box::pin(async move {
                            let _ = srv
                                .request(
                                    "turn/steer",
                                    json!({
                                        "threadId": tk,
                                        "expectedTurnId": tid,
                                        "input": [{ "type": "text", "text": content, "text_elements": [] }],
                                    }),
                                )
                                .await?;
                            Ok(())
                        })
                    }));
            }

            comp_rx.await.unwrap_or_else(|_| {
                Err(AppError::with(
                    "codexIncomplete",
                    502,
                    "The Codex CLI exited before reporting a completed turn.",
                ))
            })
        }
        .await;

        abort_task.abort();
        exit_task.abort();
        server.close(None);
        *entry.deliver_fn.lock().expect("deliver_fn") = None;
        turn_res
    }

    pub async fn steer(
        &self,
        session_id: &str,
        run_id: &str,
        p: Map<String, Value>,
    ) -> Result<Value> {
        let run = self
            .operation(session_id, "run", json!({ "id": run_id }))
            .await?;
        if p.keys().any(|k| k != "id" && k != "content") {
            return Err(AppError::invalid());
        }
        let msg_id = p
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(AppError::invalid)?;
        let content = p
            .get("content")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(AppError::invalid)?;
        let thread_id = run["threadId"].as_str().unwrap_or("");

        let entry = self.active.lock().expect("active").get(run_id).cloned();
        let deliver = entry
            .as_ref()
            .and_then(|e| e.deliver_fn.lock().expect("deliver").clone());

        let Some(deliver) = deliver else {
            return self
                .operation(
                    session_id,
                    "steerRun",
                    json!({
                        "threadId": thread_id,
                        "runId": run_id,
                        "id": msg_id,
                        "content": content,
                    }),
                )
                .await
                .map_err(|err| {
                    if err.status == 409 {
                        AppError::with(
                            "turnUnavailable",
                            409,
                            "This turn is not running. Send the message as a new turn.",
                        )
                    } else {
                        err
                    }
                });
        };

        let entry = entry.expect("entry");
        let already = entry.steered.lock().expect("steered").contains(msg_id);
        if !already {
            deliver(content.to_owned()).await?;
            entry
                .steered
                .lock()
                .expect("steered")
                .insert(msg_id.to_owned());
        }
        let saved = self
            .operation(
                session_id,
                "steerRun",
                json!({
                    "threadId": thread_id,
                    "runId": run_id,
                    "id": msg_id,
                    "content": content,
                }),
            )
            .await?;
        if let Some(steer_fn) = entry.steer_fn.lock().expect("steer_fn").clone() {
            steer_fn(msg_id.to_owned());
        }
        Ok(saved)
    }

    pub async fn answer(
        &self,
        session_id: &str,
        run_id: &str,
        p: Map<String, Value>,
    ) -> Result<Value> {
        let _ = self
            .operation(session_id, "run", json!({ "id": run_id }))
            .await?;
        let entry = self.active.lock().expect("active").get(run_id).cloned();
        let request_id = p.get("requestId").and_then(Value::as_str);
        let matches_pending = entry.as_ref().is_some_and(|e| {
            e.answer
                .lock()
                .expect("answer")
                .as_ref()
                .is_some_and(|a| Some(a.id.as_str()) == request_id)
        });
        if !matches_pending {
            return Err(AppError::with(
                "questionUnavailable",
                409,
                "This question is no longer waiting for an answer.",
            ));
        }
        let answers_obj = p
            .get("answers")
            .and_then(Value::as_object)
            .ok_or_else(AppError::invalid)?;
        let mut clean = HashMap::new();
        for (key, val) in answers_obj.iter().take(10) {
            let arr = val.as_array().ok_or_else(AppError::invalid)?;
            if arr.len() > 20 {
                return Err(AppError::invalid());
            }
            let mut items = Vec::with_capacity(arr.len());
            for item in arr {
                let s = item.as_str().ok_or_else(AppError::invalid)?;
                if utf16_len(s) > 2000 {
                    return Err(AppError::invalid());
                }
                items.push(s.to_owned());
            }
            clean.insert(slice_utf16(key, 200).to_owned(), items);
        }
        let pending = entry
            .expect("entry")
            .answer
            .lock()
            .expect("answer")
            .take();
        let Some(pending) = pending else {
            return Err(AppError::with(
                "questionUnavailable",
                409,
                "This question is no longer waiting for an answer.",
            ));
        };
        let _ = pending.tx.send(clean);
        Ok(json!({ "ok": true }))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upload(
        &self,
        session_id: &str,
        user_id: &str,
        thread_id: &str,
        attachment_id: &str,
        name: &str,
        media_type: &str,
        body: Body,
    ) -> Result<Value> {
        let _ = self
            .operation(session_id, "history", json!({ "threadId": thread_id }))
            .await?;
        if !AGENT_IMAGE_TYPES.contains(&media_type) {
            return Err(AppError::with(
                "unsupportedMediaType",
                415,
                "Attach PNG, JPEG, GIF or WebP images.",
            ));
        }
        if name.is_empty()
            || utf16_len(name) > 200
            || name
                .chars()
                .any(|c| (c as u32) < 0x20 || c == '/' || c == '\\')
        {
            return Err(AppError::invalid());
        }
        let folder = self
            .root
            .join("agent-workspaces")
            .join(user_id)
            .join(thread_id)
            .join("attachments");
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&folder)
            .await
            .map_err(|_| AppError::new("unsafeWorkspace", 409))?;
        if tokio::fs::symlink_metadata(&folder)
            .await
            .map_err(|_| AppError::new("unsafeWorkspace", 409))?
            .file_type()
            .is_symlink()
        {
            return Err(AppError::new("unsafeWorkspace", 409));
        }
        let target = folder.join(attachment_id);
        let temporary = folder.join(format!("{attachment_id}.part"));

        let write_result: Result<u64> = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
                .await
                .map_err(|_| AppError::internal())?;
            let mut stream = body.into_data_stream();
            let mut size: u64 = 0;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| AppError::internal())?;
                size += chunk.len() as u64;
                if size > AGENT_ATTACHMENT_MAX_BYTES {
                    return Err(AppError::with(
                        "requestTooLarge",
                        413,
                        "Images must be 8 MiB or smaller.",
                    ));
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|_| AppError::internal())?;
            }
            file.flush().await.map_err(|_| AppError::internal())?;
            Ok(size)
        }
        .await;

        let size = match write_result {
            Ok(0) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(AppError::invalid());
            }
            Ok(n) => n,
            Err(err) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(err);
            }
        };

        tokio::fs::rename(&temporary, &target)
            .await
            .map_err(|_| AppError::internal())?;
        let info = AgentAttachment {
            id: attachment_id.to_owned(),
            name: name.to_owned(),
            media_type: media_type.to_owned(),
            size,
        };
        let info_path = folder.join(format!("{attachment_id}.json"));
        let info_bytes = serde_json::to_vec(&info).map_err(|_| AppError::internal())?;
        let mut meta_file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&info_path)
            .await
            .map_err(|_| AppError::internal())?;
        meta_file
            .write_all(&info_bytes)
            .await
            .map_err(|_| AppError::internal())?;

        serde_json::to_value(info).map_err(|_| AppError::internal())
    }

    pub async fn attachment(
        &self,
        session_id: &str,
        user_id: &str,
        thread_id: &str,
        attachment_id: &str,
    ) -> Result<(AgentAttachment, tokio::fs::File)> {
        let _ = self
            .operation(session_id, "history", json!({ "threadId": thread_id }))
            .await?;
        let info = self.attachment_info(user_id, thread_id, attachment_id).await?;
        let path = self
            .root
            .join("agent-workspaces")
            .join(user_id)
            .join(thread_id)
            .join("attachments")
            .join(attachment_id);
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|_| AppError::not_found())?;
        Ok((info, file))
    }

    pub async fn branch(
        &self,
        session_id: &str,
        user_id: &str,
        thread_id: &str,
        commit: bool,
        blobs: &Blobs,
    ) -> Result<Value> {
        let history: AgentHistoryRow = serde_json::from_value(
            self.operation(session_id, "history", json!({ "threadId": thread_id }))
                .await?,
        )
        .map_err(|_| AppError::internal())?;
        let projects = self.operation(session_id, "projects", json!({})).await?;
        let project = projects["items"]
            .as_array()
            .and_then(|arr| {
                arr.iter()
                    .find(|item| item["id"].as_str() == Some(history.thread.project_id.as_str()))
            });
        let Some(project) = project.filter(|p| p["sourceType"].as_str() == Some("files")) else {
            return Ok(json!({ "tracked": false }));
        };
        let folder_id = project["sourceRef"].as_str().unwrap_or("").to_owned();

        let call_history = |name: &str, mut value: Map<String, Value>| {
            let db = self.db.clone();
            let sid = session_id.to_owned();
            let fid = folder_id.clone();
            let tid = thread_id.to_owned();
            let op = name.to_owned();
            async move {
                value.insert("folderId".into(), json!(fid));
                value.insert("threadId".into(), json!(tid));
                db.operation(
                    "service_history",
                    &op,
                    &HISTORY_READ_OPERATIONS,
                    json!([sid, op, Value::Object(value).to_string()]),
                )
                .await
            }
        };

        let state = match call_history("branch", Map::new()).await {
            Ok(s) => s,
            Err(err) if err.code == "notFound" => {
                return Ok(json!({ "tracked": false, "folderId": folder_id }));
            }
            Err(err) => return Err(err),
        };
        if !commit {
            let mut out = state.as_object().cloned().unwrap_or_default();
            out.insert("tracked".into(), json!(true));
            out.insert("folderId".into(), json!(folder_id));
            return Ok(Value::Object(out));
        }
        if history.runs.iter().any(|r| active_agent_run(&r.status)) {
            return Err(AppError::with(
                "runActive",
                409,
                "Wait for the agent to finish before committing its changes.",
            ));
        }
        let changes = branch_changes(&self.root, user_id, thread_id).await?;
        if changes.is_empty() {
            return Err(AppError::with(
                "nothingToCommit",
                409,
                "The agent has not changed any files in this thread.",
            ));
        }
        let content_indices: Vec<usize> = changes
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.data.as_ref().map(|_| i))
            .collect();
        let begin_files: Vec<Value> = content_indices
            .iter()
            .map(|&i| {
                let c = &changes[i];
                json!({
                    "size": c.data.as_ref().map_or(0, Vec::len),
                    "mediaType": c.media_type,
                })
            })
            .collect();
        let mut begin_arg = Map::new();
        begin_arg.insert("files".into(), Value::Array(begin_files));
        let begin_res = call_history("stageBegin", begin_arg).await?;
        let uploads: Vec<String> = begin_res["uploads"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let mut digests: HashMap<usize, (String, String)> = HashMap::new();
        for (idx, &change_idx) in content_indices.iter().enumerate() {
            let upload_id = uploads.get(idx).cloned().unwrap_or_default();
            let data = changes[change_idx].data.clone().unwrap_or_default();
            let digest = blobs.store_bytes(data).await?;
            digests.insert(change_idx, (upload_id, digest));
        }

        let files: Vec<Value> = changes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut obj = Map::new();
                obj.insert("path".into(), json!(c.path));
                obj.insert("kind".into(), json!(c.kind));
                if let Some(ref fid) = c.file_id {
                    obj.insert("fileId".into(), json!(fid));
                }
                if let Some(rev) = c.expect_revision {
                    obj.insert("expectRevision".into(), json!(rev));
                }
                if let Some((uid, dig)) = digests.get(&i) {
                    obj.insert("uploadId".into(), json!(uid));
                    obj.insert("digest".into(), json!(dig));
                }
                Value::Object(obj)
            })
            .collect();

        let mut stage_arg = Map::new();
        stage_arg.insert("title".into(), json!(history.thread.title));
        stage_arg.insert("files".into(), Value::Array(files));
        let staged = call_history("stage", stage_arg).await?;
        let mut out = staged.as_object().cloned().unwrap_or_default();
        out.insert("tracked".into(), json!(true));
        out.insert("folderId".into(), json!(folder_id));
        Ok(Value::Object(out))
    }

    pub async fn git_action(
        &self,
        session_id: &str,
        user_id: &str,
        thread_id: &str,
        p: Map<String, Value>,
    ) -> Result<Value> {
        let history: AgentHistoryRow = serde_json::from_value(
            self.operation(session_id, "history", json!({ "threadId": thread_id }))
                .await?,
        )
        .map_err(|_| AppError::internal())?;
        let work = self
            .root
            .join("agent-workspaces")
            .join(user_id)
            .join(thread_id)
            .join("work");
        let is_git = tokio::fs::symlink_metadata(work.join(".git"))
            .await
            .is_ok_and(|m| m.is_dir());
        if !is_git {
            return Ok(json!({ "available": false }));
        }
        let action = p.get("action").and_then(Value::as_str).unwrap_or("");
        if action != "status" && history.runs.iter().any(|r| active_agent_run(&r.status)) {
            return Err(AppError::with(
                "runActive",
                409,
                "Wait for the agent to finish before changing branches or committing.",
            ));
        }
        let cancel = CancelToken::with_timeout(Duration::from_secs(30));

        match action {
            "branch" => {
                static RE_BRANCH: OnceLock<Regex> = OnceLock::new();
                let re_branch = RE_BRANCH.get_or_init(|| {
                    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$").expect("regex")
                });
                let name = p.get("name").and_then(Value::as_str).unwrap_or("").trim();
                if !re_branch.is_match(name)
                    || name.contains("..")
                    || name.ends_with(".lock")
                    || name.ends_with('/')
                {
                    return Err(AppError::with(
                        "invalidInput",
                        400,
                        "Use letters, numbers, dots, dashes, underscores and slashes for branch names.",
                    ));
                }
                if git(&work, &["switch", "-c", name], cancel.clone())
                    .await
                    .is_err()
                {
                    return Err(AppError::with(
                        "branchFailed",
                        409,
                        "That branch could not be created. It may already exist.",
                    ));
                }
            }
            "commit" => {
                let message = p.get("message").and_then(Value::as_str).unwrap_or("").trim();
                if message.is_empty() || utf16_len(message) > 500 {
                    return Err(AppError::with(
                        "invalidInput",
                        400,
                        "Write a commit message of up to 500 characters.",
                    ));
                }
                git(&work, &["add", "-A"], cancel.clone()).await?;
                if git(
                    &work,
                    &[
                        "-c",
                        "user.name=Dar Agent",
                        "-c",
                        "user.email=agent@dar.invalid",
                        "commit",
                        "-m",
                        message,
                    ],
                    cancel.clone(),
                )
                .await
                .is_err()
                {
                    return Err(AppError::with(
                        "commitFailed",
                        409,
                        "Nothing to commit, or the commit could not be created.",
                    ));
                }
            }
            "status" => {}
            _ => return Err(AppError::invalid()),
        }

        let (branch_raw, status_raw, log_raw) = tokio::try_join!(
            git(&work, &["rev-parse", "--abbrev-ref", "HEAD"], cancel.clone()),
            git(&work, &["status", "--porcelain"], cancel.clone()),
            git(&work, &["log", "-5", "--format=%h%x09%s"], cancel.clone()),
        )?;
        let branch = branch_raw.trim();
        let status = status_raw.trim();
        let log = log_raw.trim();
        let changed = if status.is_empty() {
            0
        } else {
            status.split('\n').count()
        };
        let commits: Vec<Value> = if log.is_empty() {
            Vec::new()
        } else {
            log.split('\n')
                .map(|line| {
                    let (hash, subject) = line.split_once('\t').unwrap_or((line, ""));
                    json!({ "hash": hash, "subject": subject })
                })
                .collect()
        };
        Ok(json!({
            "available": true,
            "branch": branch,
            "changed": changed,
            "commits": commits,
        }))
    }

    pub async fn terminal(
        &self,
        session_id: &str,
        user_id: &str,
        thread_id: &str,
        action: &str,
        p: Map<String, Value>,
    ) -> Result<Value> {
        let _ = self
            .operation(session_id, "history", json!({ "threadId": thread_id }))
            .await?;
        let rows = p.get("rows").and_then(Value::as_f64);
        let cols = p.get("cols").and_then(Value::as_f64);
        match action {
            "open" => {
                let codex = self.codex_connection(user_id).await;
                let Some(config) = codex.config.as_ref().filter(|_| codex.status.connected) else {
                    return Err(AppError::with(
                        "codexLoginRequired",
                        409,
                        &format!(
                            "The terminal runs through the server Codex CLI. {}",
                            codex.status.message
                        ),
                    ));
                };
                assert_no_codex_rules(config).await?;
                self.terminals.open(config, user_id, thread_id, rows, cols).await
            }
            "read" => {
                let after = match p.get("after") {
                    None => 0usize,
                    Some(Value::String(s)) if s.is_empty() => 0usize,
                    Some(Value::String(s)) => s.parse::<usize>().map_err(|_| AppError::invalid())?,
                    Some(Value::Number(n)) => {
                        let f = n.as_f64().ok_or_else(AppError::invalid)?;
                        if f.fract() != 0.0 || f < 0.0 || f > 9_007_199_254_740_991.0 {
                            return Err(AppError::invalid());
                        }
                        f as usize
                    }
                    _ => return Err(AppError::invalid()),
                };
                Ok(self.terminals.read(user_id, thread_id, after).await)
            }
            "input" => {
                let data = p
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(AppError::invalid)?;
                self.terminals.write(user_id, thread_id, data).await
            }
            "resize" => Ok(self.terminals.resize(user_id, thread_id, rows, cols).await),
            "close" => Ok(self.terminals.close(user_id, thread_id)),
            _ => Err(AppError::not_found()),
        }
    }

    pub async fn delete_thread(
        &self,
        session_id: &str,
        user_id: &str,
        thread_id: &str,
    ) -> Result<Value> {
        let result = self
            .operation(session_id, "deleteThread", json!({ "threadId": thread_id }))
            .await?;
        let _ = self.terminals.close(user_id, thread_id);
        let dir = self
            .root
            .join("agent-workspaces")
            .join(user_id)
            .join(thread_id);
        let _ = tokio::fs::remove_dir_all(&dir).await;
        Ok(result)
    }

    pub async fn files(
        &self,
        session_id: &str,
        user_id: &str,
        thread_id: &str,
        query: &str,
    ) -> Result<Value> {
        let history: AgentHistoryRow = serde_json::from_value(
            self.operation(session_id, "history", json!({ "threadId": thread_id }))
                .await?,
        )
        .map_err(|_| AppError::internal())?;
        let work = self
            .root
            .join("agent-workspaces")
            .join(user_id)
            .join(thread_id)
            .join("work");
        let mut paths: Vec<String> = Vec::new();
        let source_kind: &'static str;

        let walk_ok = async {
            let meta = tokio::fs::symlink_metadata(&work).await?;
            if !meta.is_dir() {
                return Err(std::io::Error::other("not dir"));
            }
            let mut pending = VecDeque::from([String::new()]);
            let mut seen = 0usize;
            while let Some(relative) = pending.pop_front() {
                if seen >= 5000 {
                    break;
                }
                let dir = if relative.is_empty() {
                    work.clone()
                } else {
                    work.join(&relative)
                };
                let mut entries = tokio::fs::read_dir(&dir).await?;
                while let Some(entry) = entries.next_entry().await? {
                    seen += 1;
                    if seen > 5000 {
                        break;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if matches!(name.as_str(), ".git" | ".codex" | "node_modules") {
                        continue;
                    }
                    let next = if relative.is_empty() {
                        name
                    } else {
                        format!("{relative}/{name}")
                    };
                    if entry.file_type().await.is_ok_and(|ft| ft.is_dir()) {
                        pending.push_back(next);
                    } else {
                        paths.push(next);
                    }
                }
            }
            Ok::<(), std::io::Error>(())
        }
        .await;

        if walk_ok.is_ok() {
            source_kind = "workspace";
        } else {
            let project = self
                .operation(
                    session_id,
                    "source",
                    json!({ "projectId": history.thread.project_id }),
                )
                .await
                .ok()
                .and_then(|v| serde_json::from_value::<Source>(v).ok());
            if let Some(proj) = project {
                source_kind = if proj.project.source_type == "files" {
                    "files"
                } else {
                    "none"
                };
                paths = proj.files.into_iter().map(|f| f.path).collect();
            } else {
                source_kind = "none";
            }
        }

        let needle = query.to_lowercase();
        let needle_chars: Vec<char> = needle.chars().collect();
        let score = |value: &str| -> u8 {
            let lower = value.to_lowercase();
            if needle_chars.is_empty() {
                return 1;
            }
            let name = lower.rsplit('/').next().unwrap_or("");
            if name.starts_with(&needle) {
                return 3;
            }
            if lower.contains(&needle) {
                return 2;
            }
            let mut idx = 0usize;
            for ch in lower.chars() {
                if ch == needle_chars[idx] {
                    idx += 1;
                    if idx == needle_chars.len() {
                        return 1;
                    }
                }
            }
            0
        };

        let mut scored: Vec<(String, u8)> = paths
            .into_iter()
            .filter_map(|p| {
                let s = score(&p);
                (s > 0).then_some((p, s))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.len().cmp(&b.0.len()))
                .then_with(|| locale_compare(&a.0, &b.0))
        });
        let items: Vec<String> = scored.into_iter().take(50).map(|(p, _)| p).collect();
        Ok(json!({ "source": source_kind, "items": items }))
    }

    pub async fn cancel(&self, session_id: &str, id: &str) -> Result<Value> {
        let result = self
            .operation(session_id, "cancelRun", json!({ "id": id }))
            .await?;
        if let Some(entry) = self.active.lock().expect("active").get(id).cloned() {
            entry.cancel.cancel();
        }
        Ok(result)
    }
}
