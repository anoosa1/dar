//! Dar's Agent workspace (`next/client/agent/workspace.tsx`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::agent::pages::{
    AgentPage, AgentSettingsModal, AutomationsPage, PageLimitWindow, PageLimits, PageModelInfo,
    PageProjectInfo, PageProviderInfo, PageThreadInfo, PullRequestsPage, UsagePage,
};
use crate::agent::surfaces::{
    AgentScheduleEntry, ContextChip, FilesSurface, PreviewSurface, ProjectScript,
    ProjectScriptsCard, PromptStashEntry, PullRequestsSurface, SideChatSurface, SplitDiffView,
    SurfaceKind, scripts_patch,
};
use crate::agent::terminal::{MultiTerminalDrawer, ScriptLaunch};
use crate::api::{self, BrowserSession, Req, RequestError};
use crate::dates::{local_date_time, local_time};
use crate::files::types::FileTile;
use crate::git::diff::{Diff, DiffStat, count_lines, unified_hunks};
use crate::git::parts::HistoryBranch;
use crate::history::history_failure;
use crate::icons::Icon;
use crate::markdown::Markdown;
use crate::random_uuid;
use crate::settings::{AppSettingsButton, Notice};
use crate::shared::{self, Preferences};
use crate::shell::WorkspaceHeader;
use crate::ui::{
    ActionMenu, Dialog, EditorPane, Listener, MenuAction, Select, document, listen, use_body_class,
    use_floating, window,
};

const BASE: &str = "/api/v1/agent";
const AGENT_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
const AGENT_ATTACHMENT_MAX_COUNT: usize = 4;
const AGENT_ATTACHMENT_MAX_BYTES: u64 = 8 * 1024 * 1024;
const AGENT_MESSAGE_MAX_BYTES: usize = 16384;

fn provider_of(model: &str) -> &'static str {
    if model.starts_with("claude/") {
        "claude"
    } else {
        "codex"
    }
}

fn provider_display_name(provider: &str) -> &'static str {
    if provider == "claude" {
        "Claude Code"
    } else {
        "Codex"
    }
}

fn provider_name(model: &str) -> &'static str {
    provider_display_name(provider_of(model))
}

fn active_agent_run(status: &str) -> bool {
    matches!(status, "queued" | "running" | "cancelling")
}

fn effort_label(value: &str) -> String {
    if value == "xhigh" {
        return "Extra high".into();
    }
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn tokens(value: f64) -> String {
    if value >= 10000.0 {
        format!("{:.0}k", value / 1000.0)
    } else if value >= 1000.0 {
        format!("{:.1}k", value / 1000.0)
    } else {
        format!("{}", value as i64)
    }
}

fn duration(ms: f64) -> String {
    let seconds = (ms / 1000.0).round().max(1.0) as i64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {:02}s", seconds % 60);
    }
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

fn ago(time: f64, now: f64) -> String {
    let minutes = ((now - time) / 60000.0).floor() as i64;
    if minutes < 1 {
        return "now".into();
    }
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    if days < 7 {
        return format!("{days}d");
    }
    if days < 365 {
        format!("{}w", days / 7)
    } else {
        format!("{}y", days / 365)
    }
}

fn failure(err: &RequestError) -> String {
    match err.status {
        409 => "This thread changed in another window. Refresh the conversation, then retry. Your draft is retained.".into(),
        404 => "This item is unavailable, or your access changed. Refresh to continue.".into(),
        507 => "Agent storage is full for this account. No new data was saved.".into(),
        _ => {
            if err.message.is_empty() {
                "The request could not be completed.".into()
            } else {
                err.message.clone()
            }
        }
    }
}

fn merge_messages(current: &[AgentMessage], incoming: &[AgentMessage]) -> Vec<AgentMessage> {
    let mut by_id: HashMap<String, AgentMessage> = HashMap::new();
    for m in current {
        by_id.insert(m.id.clone(), m.clone());
    }
    for m in incoming {
        by_id.insert(m.id.clone(), m.clone());
    }
    let mut list: Vec<AgentMessage> = by_id.into_values().collect();
    list.sort_by_key(|m| m.sequence);
    list
}

fn encode_uri(value: &str) -> String {
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_default()
}

fn local_storage() -> Option<web_sys::Storage> {
    window().local_storage().ok().flatten()
}

fn session_storage() -> Option<web_sys::Storage> {
    window().session_storage().ok().flatten()
}

fn storage_get(storage: Option<web_sys::Storage>, key: &str) -> Option<String> {
    storage.and_then(|s| s.get_item(key).ok().flatten())
}

fn storage_set(storage: Option<web_sys::Storage>, key: &str, value: &str) -> Result<(), ()> {
    storage.ok_or(())?.set_item(key, value).map_err(|_| ())
}

fn storage_remove(storage: Option<web_sys::Storage>, key: &str) {
    if let Some(s) = storage {
        let _ = s.remove_item(key);
    }
}

fn request_animation_frame(f: impl FnOnce() + 'static) {
    let cb = Closure::once_into_js(f);
    let _ = window().request_animation_frame(cb.as_ref().unchecked_ref());
}

fn focus_prevent_scroll(el: &web_sys::HtmlElement) {
    let opts = web_sys::FocusOptions::new();
    opts.set_prevent_scroll(true);
    let _ = el.focus_with_options(&opts);
}

fn scroll_into_view_block(el: &web_sys::Element, block: web_sys::ScrollLogicalPosition) {
    let opts = web_sys::ScrollIntoViewOptions::new();
    opts.set_block(block);
    el.scroll_into_view_with_scroll_into_view_options(&opts);
}

fn is_narrow() -> bool {
    window()
        .match_media("(max-width:800px)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches())
}

fn iso_string(ms: f64) -> String {
    js_sys::Date::new(&JsValue::from_f64(ms))
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
}

fn locale_string(ms: f64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(ms));
    let Ok(func) = js_sys::Reflect::get(&date, &JsValue::from_str("toLocaleString")) else {
        return iso_string(ms);
    };
    let Ok(func) = func.dyn_into::<js_sys::Function>() else {
        return iso_string(ms);
    };
    func.call0(&date)
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| iso_string(ms))
}

async fn write_clipboard(text: &str) -> Result<(), ()> {
    let promise = window().navigator().clipboard().write_text(text);
    JsFuture::from(promise).await.map(|_| ()).map_err(|_| ())
}

fn notification_ctor() -> Option<js_sys::Function> {
    let val = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("Notification")).ok()?;
    if val.is_undefined() || val.is_null() {
        return None;
    }
    val.dyn_into::<js_sys::Function>().ok()
}

fn notification_permission() -> String {
    let Some(ctor) = notification_ctor() else {
        return "unsupported".into();
    };
    js_sys::Reflect::get(&ctor, &JsValue::from_str("permission"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| "unsupported".into())
}

async fn request_notification_permission() -> String {
    let Some(ctor) = notification_ctor() else {
        return "unsupported".into();
    };
    let Ok(req) = js_sys::Reflect::get(&ctor, &JsValue::from_str("requestPermission")) else {
        return notification_permission();
    };
    let Ok(req) = req.dyn_into::<js_sys::Function>() else {
        return notification_permission();
    };
    let Ok(promise_val) = req.call0(&ctor) else {
        return notification_permission();
    };
    let Ok(promise) = promise_val.dyn_into::<js_sys::Promise>() else {
        return notification_permission();
    };
    JsFuture::from(promise)
        .await
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_else(notification_permission)
}

fn show_notification(title: &str, body: &str, tag: &str, on_click: Option<Box<dyn FnMut()>>) {
    let Some(ctor) = notification_ctor() else {
        return;
    };
    if notification_permission() != "granted" {
        return;
    }
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &JsValue::from_str("body"), &JsValue::from_str(body));
    let _ = js_sys::Reflect::set(&opts, &JsValue::from_str("tag"), &JsValue::from_str(tag));
    let args = js_sys::Array::of2(&JsValue::from_str(title), &opts);
    let Ok(note) = js_sys::Reflect::construct(&ctor, &args) else {
        return;
    };
    if let Some(mut f) = on_click {
        let note_clone = note.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            f();
            if let Ok(close_fn) = js_sys::Reflect::get(&note_clone, &JsValue::from_str("close"))
                && let Ok(func) = close_fn.dyn_into::<js_sys::Function>()
            {
                let _ = func.call0(&note_clone);
            }
        });
        let _ = js_sys::Reflect::set(&note, &JsValue::from_str("onclick"), cb.as_ref());
        cb.forget();
    }
}

fn data_transfer_has_image(event: &web_sys::DragEvent) -> bool {
    let Some(dt) = event.data_transfer() else {
        return false;
    };
    let Ok(items) = js_sys::Reflect::get(&dt, &JsValue::from_str("items")) else {
        return false;
    };
    let len = js_sys::Reflect::get(&items, &JsValue::from_str("length"))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as u32;
    for i in 0..len {
        if let Ok(item) = js_sys::Reflect::get(&items, &JsValue::from_f64(i as f64))
            && let Ok(t) = js_sys::Reflect::get(&item, &JsValue::from_str("type"))
            && t.as_string().is_some_and(|s| s.starts_with("image/"))
        {
            return true;
        }
    }
    false
}

fn files_from_list(list: Option<web_sys::FileList>, images_only: bool) -> Vec<web_sys::File> {
    let Some(list) = list else { return Vec::new() };
    let mut out = Vec::new();
    for i in 0..list.length() {
        if let Some(file) = list.get(i) {
            if !images_only || file.type_().starts_with("image/") {
                out.push(file);
            }
        }
    }
    out
}

fn use_now(active: Signal<bool>, interval_ms: u32) -> RwSignal<f64> {
    let now = RwSignal::new(js_sys::Date::now());
    Effect::new(move |_| {
        if !active.get() {
            return;
        }
        now.set(js_sys::Date::now());
        let cb = Closure::<dyn FnMut()>::new(move || {
            now.try_set(js_sys::Date::now());
        });
        let id = window()
            .set_interval_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                interval_ms as i32,
            )
            .ok();
        let kept = SendWrapper::new((id, cb));
        on_cleanup(move || {
            let (id, _cb) = kept.take();
            if let Some(id) = id {
                window().clear_interval_with_handle(id);
            }
        });
    });
    now
}

fn set_timeout(ms: i32, f: impl FnOnce() + 'static) -> Option<(i32, Closure<dyn FnMut()>)> {
    let mut cell = Some(f);
    let cb = Closure::<dyn FnMut()>::new(move || {
        if let Some(func) = cell.take() {
            func();
        }
    });
    let id = window()
        .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), ms)
        .ok()?;
    Some((id, cb))
}

fn clear_timeout(handle: &mut Option<(i32, Closure<dyn FnMut()>)>) {
    if let Some((id, _cb)) = handle.take() {
        window().clear_timeout_with_handle(id);
    }
}

fn full_contents(diff: &str) -> Option<(Vec<&str>, Vec<&str>, Vec<&str>)> {
    let lines: Vec<&str> = diff.split('\n').collect();
    if !lines.first()?.starts_with("--- ") || !lines.get(1)?.starts_with("+++ ") {
        return None;
    }
    let header = *lines.get(2)?;
    if !matches!(header, "@@ added @@" | "@@ modified @@" | "@@ deleted @@") {
        return None;
    }
    let mut index = 3;
    let mut old = Vec::new();
    let mut next = Vec::new();
    while index < lines.len() && lines[index].starts_with('-') {
        old.push(&lines[index][1..]);
        index += 1;
    }
    if lines.get(index).copied() == Some("") {
        index += 1;
    }
    while index < lines.len() && lines[index].starts_with('+') {
        next.push(&lines[index][1..]);
        index += 1;
    }
    if index < lines.len() && !(index == lines.len() - 1 && lines[index].is_empty()) {
        return None;
    }
    if old.last().copied() == Some("") {
        old.pop();
    }
    if next.last().copied() == Some("") {
        next.pop();
    }
    Some((lines[0..2].to_vec(), old, next))
}

fn unified_patch(diff: &str) -> String {
    match full_contents(diff) {
        Some((head, old, next)) => unified_hunks(&old, &next, &head),
        None => diff.to_owned(),
    }
}

fn diff_counts(diff: &str) -> (usize, usize) {
    if let Some((_, old, next)) = full_contents(diff) {
        return count_lines(&old, &next);
    }
    let mut added = 0;
    let mut removed = 0;
    for line in diff.split('\n') {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

fn snooze_presets() -> Vec<(&'static str, f64)> {
    let now_date = js_sys::Date::new_0();
    let now_ms = now_date.get_time();
    let at = |days: i32, hour: u32| -> f64 {
        let d = js_sys::Date::new(&JsValue::from_f64(now_ms));
        d.set_date((d.get_date() as i32 + days) as u32);
        d.set_hours(hour);
        d.set_minutes(0);
        d.set_seconds(0);
        d.set_milliseconds(0);
        d.get_time()
    };
    let evening_today = at(0, 18);
    let evening = if evening_today > now_ms + 30.0 * 60000.0 {
        evening_today
    } else {
        at(1, 18)
    };
    let day = now_date.get_day() as i32;
    let delta = (8 - day) % 7;
    let monday_days = if delta == 0 { 7 } else { delta };
    vec![
        ("In 1 hour", now_ms + 3600000.0),
        ("In 3 hours", now_ms + 3.0 * 3600000.0),
        ("This evening", evening),
        ("Tomorrow morning", at(1, 9)),
        ("Next week", at(monday_days, 9)),
    ]
}

fn wake_label(until: f64) -> String {
    local_date_time(
        until,
        &[
            ("weekday", "short"),
            ("hour", "numeric"),
            ("minute", "2-digit"),
        ],
    )
}

fn markdown_thread(thread: &AgentThread, project: &str, items: &[AgentMessage]) -> String {
    let mut lines = vec![
        format!("# {}", thread.title),
        String::new(),
        format!("Project: {project}"),
        String::new(),
    ];
    for item in items {
        let who = if item.role == "user" { "You" } else { "Agent" };
        lines.push(format!(
            "## {who} \u{00b7} {}",
            locale_string(item.created_at)
        ));
        lines.push(String::new());
        lines.push(item.content.clone());
        lines.push(String::new());
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProject {
    id: String,
    owner_id: String,
    name: String,
    created_at: f64,
    source_type: String,
    source_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentThread {
    id: String,
    owner_id: String,
    project_id: String,
    title: String,
    revision: u64,
    created_at: f64,
    modified_at: f64,
    #[serde(default)]
    run_status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentMessage {
    id: String,
    // Shared history leaves out who owns the thread.
    #[serde(default)]
    owner_id: String,
    #[serde(default)]
    thread_id: String,
    sequence: u64,
    role: String,
    content: String,
    created_at: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRunSummary {
    id: String,
    thread_id: String,
    message_id: String,
    model: String,
    status: String,
    created_at: f64,
    modified_at: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentHistory {
    thread: AgentThread,
    items: Vec<AgentMessage>,
    next_before: Option<u64>,
    #[serde(default)]
    runs: Vec<AgentRunSummary>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentChange {
    path: String,
    kind: String,
    diff: String,
    #[serde(default)]
    file_id: Option<String>,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    media_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentStepItem {
    text: String,
    #[serde(default)]
    done: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentStep {
    id: String,
    kind: String,
    label: String,
    status: String,
    #[serde(default)]
    exit_code: Option<i64>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    items: Option<Vec<AgentStepItem>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentUsage {
    input: f64,
    #[serde(default)]
    cached: f64,
    output: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentAttachment {
    id: String,
    name: String,
    #[allow(dead_code)]
    media_type: String,
    #[allow(dead_code)]
    size: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentQuestionOption {
    label: String,
    #[serde(default)]
    description: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentQuestion {
    id: String,
    #[serde(default)]
    header: String,
    question: String,
    #[serde(default)]
    options: Vec<AgentQuestionOption>,
    #[serde(default)]
    other: bool,
    #[serde(default)]
    secret: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentPending {
    id: String,
    questions: Vec<AgentQuestion>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRunDocument {
    #[serde(default)]
    output: String,
    #[serde(default)]
    terminal: String,
    #[serde(default)]
    activity: Vec<String>,
    #[serde(default)]
    changes: Vec<AgentChange>,
    #[serde(default)]
    error: String,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    steps: Option<Vec<AgentStep>>,
    #[serde(default)]
    usage: Option<AgentUsage>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    attachments: Option<Vec<AgentAttachment>>,
    #[serde(default)]
    steered: Option<Vec<String>>,
    #[serde(default)]
    pending: Option<AgentPending>,
    #[serde(default)]
    context_window: Option<f64>,
    #[serde(default)]
    context_used: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRun {
    id: String,
    thread_id: String,
    message_id: String,
    model: String,
    status: String,
    created_at: f64,
    modified_at: f64,
    document: AgentRunDocument,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentModel {
    id: String,
    name: String,
    #[serde(default)]
    is_default: bool,
    #[serde(default)]
    efforts: Option<Vec<String>>,
    #[serde(default)]
    default_effort: Option<String>,
    #[serde(default)]
    provider: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProviderStatus {
    id: String,
    name: String,
    #[serde(default)]
    configured: bool,
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    message: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProvider {
    #[serde(default)]
    configured: bool,
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    models: Vec<AgentModel>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    providers: Option<Vec<AgentProviderStatus>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentLimitWindow {
    label: String,
    used_percent: f64,
    #[allow(dead_code)]
    #[serde(default)]
    window_minutes: Option<f64>,
    #[serde(default)]
    resets_at: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentLimits {
    #[serde(default)]
    available: bool,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    windows: Vec<AgentLimitWindow>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchState {
    #[serde(default)]
    tracked: bool,
    #[serde(default)]
    folder_id: Option<String>,
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default)]
    branch: Option<HistoryBranch>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitState {
    #[serde(default)]
    available: bool,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    changed: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedThreadInfo {
    id: String,
    title: String,
    revision: u64,
    #[allow(dead_code)]
    modified_at: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedThread {
    share_id: String,
    thread: SharedThreadInfo,
    owner: String,
    project: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareRow {
    id: String,
    username: String,
    #[serde(default)]
    display_name: String,
    #[allow(dead_code)]
    created_at: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettledEntry {
    id: String,
    revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnoozedEntry {
    id: String,
    #[serde(serialize_with = "whole_ms")]
    until: f64,
}

/// Times are whole milliseconds on the wire, as `Date.now()` gives them.
fn whole_ms<S: serde::Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_i64(value.round() as i64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Threads,
    Conversation,
    Details,
    Changes,
    Terminal,
}

impl View {
    fn as_str(self) -> &'static str {
        match self {
            View::Threads => "threads",
            View::Conversation => "conversation",
            View::Details => "details",
            View::Changes => "changes",
            View::Terminal => "terminal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Layout {
    nav: bool,
    details: bool,
    review: bool,
    terminal: bool,
    sidebar_width: f64,
    review_width: f64,
}

fn saved_layout(user_id: &str) -> Layout {
    let mut defaults = Layout {
        nav: true,
        details: true,
        review: false,
        terminal: false,
        sidebar_width: 256.0,
        review_width: 560.0,
    };
    if let Some(raw) = storage_get(local_storage(), &format!("dar.agent.layout.{user_id}"))
        && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&raw)
    {
        if let Some(v) = map.get("nav").and_then(Value::as_bool) {
            defaults.nav = v;
        }
        if let Some(v) = map.get("details").and_then(Value::as_bool) {
            defaults.details = v;
        }
        if let Some(v) = map.get("review").and_then(Value::as_bool) {
            defaults.review = v;
        }
        if let Some(v) = map.get("terminal").and_then(Value::as_bool) {
            defaults.terminal = v;
        }
        if let Some(v) = map.get("sidebarWidth").and_then(Value::as_f64) {
            defaults.sidebar_width = v;
        }
        if let Some(v) = map.get("reviewWidth").and_then(Value::as_f64) {
            defaults.review_width = v;
        }
    }
    defaults
}

#[derive(Clone, Copy, Debug)]
struct ScrollPosition {
    top: i32,
    following: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct PickerSelection {
    id: String,
    name: String,
    kind: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct PickerCrumb {
    id: String,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PickerPage {
    folder: Option<PickerSelection>,
    ancestors: Vec<PickerCrumb>,
    items: Vec<PickerSelection>,
    next_cursor: Option<String>,
}

// Agent preferences helpers (stored inside `shared::Preferences::rest`)
fn pref_string_vec(prefs: &Option<Preferences>, key: &str) -> Vec<String> {
    prefs
        .as_ref()
        .and_then(|p| p.rest.get(key))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn pref_settled(prefs: &Option<Preferences>) -> Vec<SettledEntry> {
    prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentSettledThreads"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn pref_snoozed(prefs: &Option<Preferences>) -> Vec<SnoozedEntry> {
    prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentSnoozed"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn pref_send_key(prefs: &Option<Preferences>) -> &'static str {
    match prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentSendKey"))
        .and_then(Value::as_str)
    {
        Some("modEnter") => "modEnter",
        _ => "enter",
    }
}

fn pref_grouping(prefs: &Option<Preferences>) -> &'static str {
    match prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentGrouping"))
        .and_then(Value::as_str)
    {
        Some("project") => "project",
        _ => "recent",
    }
}

fn pref_notify(prefs: &Option<Preferences>) -> bool {
    prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentNotify"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn pref_follow_up_mode(prefs: &Option<Preferences>) -> &'static str {
    match prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentFollowUpMode"))
        .and_then(Value::as_str)
    {
        Some("queue") => "queue",
        _ => "steer",
    }
}

fn pref_auto_settle(prefs: &Option<Preferences>) -> bool {
    prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentAutoSettle"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn pref_diff_view(prefs: &Option<Preferences>) -> &'static str {
    match prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentDiffView"))
        .and_then(Value::as_str)
    {
        Some("split") => "split",
        _ => "unified",
    }
}

fn pref_prompt_stash(prefs: &Option<Preferences>) -> Vec<PromptStashEntry> {
    prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentPromptStash"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn pref_project_scripts_map(prefs: &Option<Preferences>) -> serde_json::Map<String, Value> {
    prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentProjectScripts"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn pref_schedules(prefs: &Option<Preferences>) -> Vec<AgentScheduleEntry> {
    prefs
        .as_ref()
        .and_then(|p| p.rest.get("agentSchedules"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

#[derive(Clone)]
struct SlashCommand {
    name: String,
    hint: String,
    argument: Option<Vec<String>>,
    run: Callback<String>,
}

struct SlashControls {
    set_effort: Callback<String>,
    efforts: Vec<String>,
}

// ---------------------------------------------------------------------------
// ProjectMark, AgentFolderPicker, ProjectEditor, ThreadChooser
// ---------------------------------------------------------------------------

#[component]
fn ProjectMark(#[prop(into)] name: String) -> impl IntoView {
    let mut hash: u32 = 0;
    for ch in name.chars() {
        hash = hash.wrapping_mul(31).wrapping_add(ch as u32);
    }
    let initial = name
        .trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    view! { <span class="agent-project-mark" data-tint=(hash % 5).to_string() aria-hidden="true">{initial}</span> }
}

/// Inline Files folder picker (`DarFilePicker` with `mode="folder"` and `allowRoot=false`).
#[component]
fn AgentFolderPicker(
    #[prop(into)] disabled: Signal<bool>,
    on_change: Callback<Option<PickerSelection>>,
) -> impl IntoView {
    let parent = RwSignal::new(String::new());
    let cursor = RwSignal::new(String::new());
    let previous: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let page: RwSignal<Option<PickerPage>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let attempt = RwSignal::new(0u32);
    let query = RwSignal::new(String::new());
    let selected: RwSignal<Option<PickerSelection>> = RwSignal::new(None);
    let abort_ctrl: StoredValue<RefCell<Option<web_sys::AbortController>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(None));

    let here = |value: &Option<PickerPage>| -> Option<PickerSelection> {
        value
            .as_ref()
            .and_then(|p| p.folder.clone())
            .map(|f| PickerSelection {
                kind: "folder".into(),
                ..f
            })
    };

    Effect::new(move |_| {
        attempt.track();
        let p = parent.get();
        let c = cursor.get();
        abort_ctrl.with_value(|cell| {
            if let Some(prev) = cell.borrow_mut().take() {
                prev.abort();
            }
        });
        let controller = web_sys::AbortController::new().expect("AbortController");
        let signal = controller.signal();
        abort_ctrl.with_value(|cell| *cell.borrow_mut() = Some(controller));
        page.set(None);
        error.set(String::new());
        selected.set(None);
        {
            on_change.try_run(None);
        };
        let url = format!(
            "/api/v1/files/picker?parentId={}&cursor={}&kind=folder",
            encode_uri(&p),
            encode_uri(&c)
        );
        spawn_local(async move {
            match api::request::<PickerPage>(
                &url,
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(value) => {
                    if signal.aborted() {
                        return;
                    }
                    let dest = value.folder.clone().map(|f| PickerSelection {
                        kind: "folder".into(),
                        ..f
                    });
                    page.try_set(Some(value));
                    {
                        on_change.try_run(dest);
                    };
                }
                Err(e) => {
                    if !signal.aborted() {
                        error.try_set(if e.message.is_empty() {
                            "Folders could not be loaded.".into()
                        } else {
                            e.message
                        });
                    }
                }
            }
        });
    });

    on_cleanup(move || {
        abort_ctrl.with_value(|cell| {
            if let Some(prev) = cell.borrow_mut().take() {
                prev.abort();
            }
        });
    });

    let browse = move |id: String| {
        parent.set(id);
        cursor.set(String::new());
        previous.set(Vec::new());
        query.set(String::new());
    };

    let choose = move |item: PickerSelection| {
        let next = if selected.with_untracked(|s| s.as_ref().is_some_and(|cur| cur.id == item.id)) {
            None
        } else {
            Some(item)
        };
        selected.set(next.clone());
        {
            on_change.try_run(next.or_else(|| page.with_untracked(|p| here(p))));
        };
    };

    let items = Memo::new(move |_| {
        let needle = query.get().trim().to_lowercase();
        page.with(|p| {
            p.as_ref()
                .map(|p| {
                    p.items
                        .iter()
                        .filter(|item| item.name.to_lowercase().contains(&needle))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });

    let up = move || -> Option<String> {
        page.with(|p| {
            p.as_ref().and_then(|p| {
                p.folder
                    .as_ref()
                    .map(|_| p.ancestors.last().map(|a| a.id.clone()).unwrap_or_default())
            })
        })
    };

    let destination = move || selected.get().or_else(|| page.with(|p| here(p)));

    view! {
        <section class="dar-file-picker" aria-label="Dar folder picker" aria-busy=move || (page.with(Option::is_none) && error.with(String::is_empty)).to_string()>
            <div class="dar-picker-head">
                <button type="button" class="icon-button" aria-label="Up one folder" title="Up one folder" disabled=move || disabled.get() || up().is_none() on:click=move |_| if let Some(id) = up() { browse(id); }><Icon name="back"/></button>
                <nav class="dar-picker-path" aria-label="Folder path">
                    {move || {
                        let current = page.get();
                        let has_folder = current.as_ref().and_then(|p| p.folder.as_ref()).is_some();
                        view! {
                            {if has_folder {
                                view! { <button type="button" disabled=move || disabled.get() on:click=move |_| browse(String::new())>"All files"</button> }.into_any()
                            } else {
                                view! { <strong aria-current="location">"All files"</strong> }.into_any()
                            }}
                            {current.as_ref().map(|p| p.ancestors.iter().map(|part| {
                                let id = part.id.clone();
                                let name = part.name.clone();
                                view! { <span><Icon name="chevron"/><button type="button" disabled=move || disabled.get() on:click=move |_| browse(id.clone())>{name}</button></span> }
                            }).collect_view())}
                            {current.as_ref().and_then(|p| p.folder.as_ref()).map(|folder| {
                                let name = folder.name.clone();
                                view! { <span><Icon name="chevron"/><strong aria-current="location">{name}</strong></span> }
                            })}
                        }
                    }}
                </nav>
            </div>
            <div class="dar-picker-search">
                <Icon name="search"/>
                <input type="search" aria-label="Filter folders in this folder" placeholder="Filter folders" prop:value=move || query.get() disabled=move || disabled.get() || page.with(Option::is_none)
                    on:input=move |e| query.set(event_target_value(&e))
                    on:keydown=move |e| if e.key() == "Enter" { e.prevent_default(); }/>
            </div>
            {move || {
                if !error.with(String::is_empty) {
                    view! { <div class="dar-picker-state" role="alert"><p class="error">{error.get()}</p><button type="button" class="secondary-button" on:click=move |_| attempt.update(|n| *n += 1)>"Retry"</button></div> }.into_any()
                } else if page.with(Option::is_none) {
                    view! { <p class="dar-picker-state muted" role="status">"Loading folders\u{2026}"</p> }.into_any()
                } else {
                    let list_label = page.with(|p| p.as_ref().and_then(|p| p.folder.as_ref()).map(|f| f.name.clone()).unwrap_or_else(|| "All files".into()));
                    let next_cursor = page.with(|p| p.as_ref().and_then(|p| p.next_cursor.clone()));
                    let list_items = items.get();
                    let is_empty = list_items.is_empty();
                    view! {
                        <ul class="dar-picker-list" aria-label=list_label>
                            {list_items.into_iter().map(|item| {
                                let pressed = selected.with(|s| s.as_ref().is_some_and(|cur| cur.id == item.id));
                                let is_folder = item.kind == "folder";
                                let id_dbl = item.id.clone();
                                let id_key = item.id.clone();
                                let id_open = item.id.clone();
                                let item_choose = item.clone();
                                let name_label = item.name.clone();
                                view! {
                                    <li class=if pressed { "is-selected" } else { "" }>
                                        <button type="button" class="dar-picker-row" disabled=move || disabled.get() aria-pressed=pressed.to_string()
                                            on:click=move |_| choose(item_choose.clone())
                                            on:dblclick=move |_| if is_folder { browse(id_dbl.clone()); }
                                            on:keydown=move |e| if is_folder && (e.key() == "ArrowRight" || (e.key() == "Enter" && e.shift_key())) { e.prevent_default(); browse(id_key.clone()); }>
                                            <FileTile kind=item.kind.clone() name=item.name.clone() media_type=""/>
                                            <span>{item.name.clone()}</span>
                                            {pressed.then(|| view! { <Icon name="check"/> })}
                                        </button>
                                        {is_folder.then(|| view! {
                                            <button type="button" class="icon-button dar-picker-open" aria-label=format!("Open {name_label}") title="Open folder" disabled=move || disabled.get() on:click=move |_| browse(id_open.clone())><Icon name="chevron"/></button>
                                        })}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                        {is_empty.then(|| view! { <p class="dar-picker-state muted">{if query.with(String::is_empty) { "No folders here." } else { "No matching folders here." }}</p> })}
                        {(previous.with(|v| !v.is_empty()) || next_cursor.is_some()).then(|| {
                            let has_next = next_cursor.is_some();
                            let next_c = next_cursor.clone();
                            view! {
                                <div class="dar-picker-pages">
                                    <button type="button" class="text-button" disabled=move || disabled.get() || previous.with(Vec::is_empty) on:click=move |_| {
                                        let prev_val = previous.with_untracked(|v| v.last().cloned().unwrap_or_default());
                                        cursor.set(prev_val);
                                        previous.update(|v| { v.pop(); });
                                    }>"Previous"</button>
                                    <button type="button" class="text-button" disabled=move || disabled.get() || !has_next on:click=move |_| {
                                        if let Some(ref nc) = next_c {
                                            let cur = cursor.get_untracked();
                                            previous.update(|v| v.push(cur));
                                            cursor.set(nc.clone());
                                        }
                                    }>"Next"</button>
                                </div>
                            }
                        })}
                    }.into_any()
                }
            }}
            <p class="dar-picker-selection" role="status">
                {move || match destination() {
                    Some(dest) => view! { <Icon name=if dest.kind == "folder" { "folder" } else { "file" }/><span>"Destination: "<strong>{dest.name}</strong></span> }.into_any(),
                    None => view! { <span>"Open a folder to use it."</span> }.into_any(),
                }}
            </p>
        </section>
    }
}

#[component]
fn ProjectEditor(
    session: BrowserSession,
    on_created: Callback<AgentProject>,
    on_close: Callback<()>,
) -> impl IntoView {
    let kind = RwSignal::new("files".to_owned());
    let repository = RwSignal::new(String::new());
    let folder: RwSignal<Option<PickerSelection>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let id = StoredValue::new(random_uuid());
    let csrf = StoredValue::new(session.csrf_token);
    let controller =
        StoredValue::new_local(web_sys::AbortController::new().expect("AbortController"));
    on_cleanup(move || controller.with_value(|c| c.abort()));

    let name = Memo::new(move |_| {
        if kind.get() == "files" {
            folder.get().map(|f| f.name).unwrap_or_default()
        } else {
            let trimmed = repository.get();
            let trimmed = trimmed.trim().trim_end_matches('/');
            let last = trimmed.rsplit(['/', ':']).next().unwrap_or("");
            last.strip_suffix(".git").unwrap_or(last).to_owned()
        }
    });

    let dirty = Signal::derive(move || !repository.with(String::is_empty));

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        busy.set(true);
        error.set(String::new());
        let k = kind.get_untracked();
        let n = name.get_untracked();
        let source_ref = if k == "git" {
            repository.get_untracked()
        } else {
            folder.get_untracked().map(|f| f.id).unwrap_or_default()
        };
        let body = json!({
            "id": id.get_value(),
            "name": n,
            "sourceType": k,
            "sourceRef": source_ref,
        })
        .to_string();
        let signal = controller.with_value(|c| c.signal());
        let token = csrf.get_value();
        spawn_local(async move {
            let result = api::request::<AgentProject>(
                &format!("{BASE}/projects"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await;
            if signal.aborted() {
                return;
            }
            match result {
                Ok(project) => {
                    on_created.try_run(project);
                }
                Err(e) => {
                    error.try_set(failure(&e));
                }
            }
            busy.try_set(false);
        });
    };

    view! {
        <EditorPane title="Add project" on_close=on_close dirty=dirty>
            <form class="agent-project-form" on:submit=submit>
                <label>
                    "Project source"
                    <Select
                        options=vec![("git".into(), "Git repository".into()), ("files".into(), "Dar Files folder".into())]
                        value=kind
                    />
                </label>
                {move || if kind.get() == "git" {
                    view! {
                        <label>
                            "Repository URL"
                            <input required type="url" autocapitalize="none" spellcheck="false" placeholder="https://github.com/you/project.git"
                                prop:value=move || repository.get() maxlength="2048" disabled=move || busy.get()
                                on:input=move |e| repository.set(event_target_value(&e))/>
                        </label>
                        <p class="muted small">"Use a public HTTPS repository on an administrator-enabled host."</p>
                    }.into_any()
                } else {
                    view! {
                        <AgentFolderPicker disabled=Signal::derive(move || busy.get()) on_change=Callback::new(move |sel| folder.set(sel))/>
                    }.into_any()
                }}
                {move || {
                    let n = name.get();
                    (!n.is_empty()).then(|| view! { <p class="muted small">"Project: "<strong>{n}</strong></p> })
                }}
                {move || {
                    let e = error.get();
                    (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> })
                }}
                <div class="dialog-actions">
                    <button type="button" class="secondary-button" on:click=move |_| { on_close.try_run(()); } disabled=move || busy.get()>"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get() || name.with(String::is_empty) || (kind.get() == "files" && folder.with(Option::is_none))>
                        {move || if busy.get() { "Adding\u{2026}" } else { "Add project" }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}

#[component]
fn ThreadChooser(
    #[prop(into)] projects: Signal<Vec<AgentProject>>,
    #[prop(into)] busy: Signal<bool>,
    #[prop(into)] error: Signal<String>,
    on_choose: Callback<AgentProject>,
    on_add: Callback<()>,
    on_close: Callback<()>,
) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let matches = Memo::new(move |_| {
        let needle = query.get().to_lowercase();
        projects.with(|list| {
            list.iter()
                .filter(|p| p.name.to_lowercase().contains(&needle))
                .cloned()
                .collect::<Vec<_>>()
        })
    });

    view! {
        <EditorPane title="Choose project" on_close=on_close>
            <div class="agent-project-chooser">
                <label class="sr-only" for="agent-project-search">"Find a project"</label>
                <input id="agent-project-search" type="search" placeholder="Find a project\u{2026}" prop:value=move || query.get() on:input=move |e| query.set(event_target_value(&e))/>
                <h3>"Projects"</h3>
                {move || matches.get().into_iter().map(|project| {
                    let kind_label = if project.source_type == "files" { "Dar Files" } else { "Git repository" };
                    let aria = format!("{} {kind_label}", project.name);
                    let chosen = project.clone();
                    view! {
                        <button class="agent-project-choice" aria-label=aria disabled=move || busy.get() on:click=move |_| { on_choose.try_run(chosen.clone()); }>
                            <ProjectMark name=project.name.clone()/>
                            <span><strong>{project.name.clone()}</strong><small>{kind_label}</small></span>
                            <Icon name="chevron"/>
                        </button>
                    }
                }).collect_view()}
                {move || matches.with(Vec::is_empty).then(|| view! {
                    <p class="muted">{if projects.with(Vec::is_empty) { "Add a project to start working." } else { "No matching projects." }}</p>
                })}
                <button class="text-button" disabled=move || busy.get() on:click=move |_| { on_add.try_run(()); }><Icon name="plus"/>"Add project"</button>
                {move || {
                    let e = error.get();
                    (!e.is_empty()).then(|| view! { <p class="error" role="alert">{e}</p> })
                }}
            </div>
        </EditorPane>
    }
}

// ---------------------------------------------------------------------------
// ModelPicker & use_model_choice
// ---------------------------------------------------------------------------

#[component]
fn ModelPicker(
    #[prop(into)] models: Signal<Vec<AgentModel>>,
    #[prop(into)] value: Signal<String>,
    on_change: Callback<String>,
    #[prop(into)] effort: Signal<String>,
    on_effort: Callback<String>,
    #[prop(into)] disabled: Signal<bool>,
    #[prop(optional, into)] starred: Option<Signal<Vec<String>>>,
    #[prop(optional)] on_toggle_star: Option<Callback<String>>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let provider_tab = RwSignal::new("all");
    let active = RwSignal::new(0usize);
    let root = NodeRef::<html::Div>::new();
    let trigger = NodeRef::<html::Button>::new();
    let search = NodeRef::<html::Input>::new();
    let list = NodeRef::<html::Ul>::new();
    let popover = NodeRef::<html::Div>::new();

    use_floating(open.into(), trigger, popover, false, 6.0);

    let shown = Memo::new(move |_| {
        let needle = query.get().trim().to_lowercase();
        let tab = provider_tab.get();
        let star_list = starred.map(|s| s.get()).unwrap_or_default();
        models.with(|all| {
            let mut filtered: Vec<AgentModel> = all
                .iter()
                .filter(|m| {
                    let prov_id = m.provider.as_deref().unwrap_or("codex");
                    if tab != "all" && prov_id != tab {
                        return false;
                    }
                    let prov = provider_display_name(prov_id);
                    format!("{} {} {prov}", m.name, m.id)
                        .to_lowercase()
                        .contains(&needle)
                })
                .cloned()
                .collect();
            if !star_list.is_empty() {
                filtered.sort_by_key(|m| if star_list.contains(&m.id) { 0u8 } else { 1u8 });
            }
            filtered
        })
    });

    let selected = Memo::new(move |_| {
        let current = value.get();
        models.with(|all| all.iter().find(|m| m.id == current).cloned())
    });

    let grouped = Memo::new(move |_| {
        models.with(|all| {
            let set: HashSet<&str> = all
                .iter()
                .map(|m| m.provider.as_deref().unwrap_or("codex"))
                .collect();
            set.len() > 1
        })
    });

    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        query.set(String::new());
        let current = value.get_untracked();
        let idx =
            models.with_untracked(|all| all.iter().position(|m| m.id == current).unwrap_or(0));
        active.set(idx);
        request_animation_frame(move || {
            if let Some(el) = search.get_untracked() {
                let _ = el.focus();
            } else if let Some(el) = list.get_untracked() {
                let _ = el.focus();
            }
        });
        listen(&document(), "pointerdown", false, move |event| {
            let target = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            if let Some(root_el) = root.get_untracked()
                && !root_el.contains(target.as_ref())
            {
                open.set(false);
            }
        });
    });

    Effect::new(move |_| {
        active.track();
        if !open.get() {
            return;
        }
        if let Some(ul) = list.get()
            && let Ok(Some(el)) = ul.query_selector("[data-active]")
        {
            scroll_into_view_block(&el, web_sys::ScrollLogicalPosition::Nearest);
        }
    });

    let choose = move |id: String| {
        let prev_has_efforts = selected.with_untracked(|s| {
            s.as_ref()
                .and_then(|m| m.efforts.as_ref())
                .is_some_and(|e| !e.is_empty())
        });
        let next_has_efforts = models.with_untracked(|all| {
            all.iter()
                .find(|m| m.id == id)
                .and_then(|m| m.efforts.as_ref())
                .is_some_and(|e| !e.is_empty())
        });
        {
            on_change.try_run(id);
        };
        if !prev_has_efforts || !next_has_efforts {
            open.set(false);
            if let Some(el) = trigger.get_untracked() {
                let _ = el.focus();
            }
        }
    };

    let on_keydown = move |event: web_sys::KeyboardEvent| {
        let key = event.key();
        let target_el = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok());
        if key == "Escape" {
            event.prevent_default();
            event.stop_propagation();
            open.set(false);
            if let Some(el) = trigger.get_untracked() {
                let _ = el.focus();
            }
        } else if (key == "ArrowDown" || key == "ArrowUp")
            && target_el
                .as_ref()
                .and_then(|e| e.closest(".agent-effort").ok().flatten())
                .is_none()
        {
            event.prevent_default();
            let len = shown.with_untracked(Vec::len);
            if len > 0 {
                let delta = if key == "ArrowDown" { 1 } else { len - 1 };
                active.update(|idx| *idx = (*idx + delta) % len);
            } else {
                active.set(0);
            }
        } else if key == "Enter"
            && target_el
                .as_ref()
                .and_then(|e| e.closest("button").ok().flatten())
                .is_none()
        {
            let chosen =
                shown.with_untracked(|list| list.get(active.get_untracked()).map(|m| m.id.clone()));
            if let Some(id) = chosen {
                event.prevent_default();
                {
                    on_change.try_run(id);
                };
                open.set(false);
                if let Some(el) = trigger.get_untracked() {
                    let _ = el.focus();
                }
            }
        }
    };

    let trigger_aria = move || {
        let sel = selected.get();
        let prov = sel
            .as_ref()
            .map(|m| provider_display_name(m.provider.as_deref().unwrap_or("codex")))
            .unwrap_or("Agent");
        let name = sel.as_ref().map(|m| m.name.as_str()).unwrap_or("none");
        let eff = effort.get();
        if eff.is_empty() {
            format!("{prov} model: {name}")
        } else {
            format!("{prov} model: {name}, {} reasoning", effort_label(&eff))
        }
    };

    view! {
        <div class="agent-model-picker" node_ref=root>
            <button type="button" node_ref=trigger class="agent-model-trigger"
                aria-label=trigger_aria
                aria-haspopup="dialog"
                aria-expanded=move || open.get().to_string()
                disabled=move || disabled.get()
                on:click=move |_| open.update(|v| *v = !*v)>
                <Icon name="agent"/>
                <span>{move || selected.with(|s| s.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| "Choose model".into()))}</span>
                {move || {
                    let eff = effort.get();
                    (!eff.is_empty()).then(|| view! { <span class="agent-effort-chip">{effort_label(&eff)}</span> })
                }}
                <Icon name="chevron"/>
            </button>
            {move || open.get().then(|| {
                let many = models.with(|m| m.len() > 6);
                let items = shown.get();
                let is_grouped = grouped.get();
                let current_val = value.get();
                let cur_active = active.get();
                let activedesc = items.get(cur_active).map(|_| format!("agent-model-{cur_active}"));
                let efforts = selected.with(|s| s.as_ref().and_then(|m| m.efforts.clone()).unwrap_or_default());
                let cur_effort = effort.get();
                let star_list = starred.map(|s| s.get()).unwrap_or_default();
                view! {
                    <div node_ref=popover class="agent-model-popover" role="dialog" aria-label="Model and reasoning" on:keydown=on_keydown>
                        {is_grouped.then(|| view! {
                            <div class="agent-model-tabs" role="tablist" aria-label="Providers">
                                {[("all", "All"), ("codex", "Codex"), ("claude", "Claude Code")].into_iter().map(|(k, lbl)| view! {
                                    <button type="button" role="tab" aria-selected=move || (provider_tab.get() == k).to_string()
                                        on:click=move |_| { provider_tab.set(k); active.set(0); }>{lbl}</button>
                                }).collect_view()}
                            </div>
                        })}
                        {many.then(|| view! {
                            <input node_ref=search type="search" aria-label="Search models" placeholder="Search models" prop:value=move || query.get()
                                on:input=move |e| { query.set(event_target_value(&e)); active.set(0); }/>
                        })}
                        <ul node_ref=list role="listbox" aria-label="Models" tabindex="-1" aria-activedescendant=activedesc>
                            {items.iter().enumerate().map(|(index, model)| {
                                let prev_prov = index.checked_sub(1).and_then(|i| items.get(i)).and_then(|m| m.provider.as_deref());
                                let show_group = is_grouped && provider_tab.get() == "all" && model.provider.as_deref() != prev_prov;
                                let group_name = provider_display_name(model.provider.as_deref().unwrap_or("codex"));
                                let is_sel = model.id == current_val;
                                let is_act = index == cur_active;
                                let is_starred = star_list.contains(&model.id);
                                let id_choose = model.id.clone();
                                let id_star = model.id.clone();
                                view! {
                                    {show_group.then(|| view! { <li class="agent-model-group" role="presentation">{group_name}</li> })}
                                    <li id=format!("agent-model-{index}") role="option" aria-selected=is_sel.to_string() data-active=is_act.then_some("true")
                                        on:pointerenter=move |_| active.set(index)
                                        on:click=move |_| choose(id_choose.clone())>
                                        {on_toggle_star.map(|cb| view! {
                                            <button type="button" class=if is_starred { "agent-model-star is-on" } else { "agent-model-star" }
                                                aria-label=if is_starred { "Unstar model" } else { "Star model" }
                                                on:click=move |e| {
                                                    e.stop_propagation();
                                                    cb.try_run(id_star.clone());
                                                }>
                                                <Icon name="star"/>
                                            </button>
                                        })}
                                        <span>{model.name.clone()}</span>
                                        {model.is_default.then(|| view! { <small>"Default"</small> })}
                                        {is_sel.then(|| view! { <Icon name="check"/> })}
                                    </li>
                                }
                            }).collect_view()}
                            {items.is_empty().then(|| view! { <li class="muted" role="presentation">"No matching models"</li> })}
                        </ul>
                        {(!efforts.is_empty()).then(|| {
                            let all_efforts = efforts.clone();
                            view! {
                                <div class="agent-effort" role="radiogroup" aria-label="Reasoning">
                                    <span>"Reasoning"</span>
                                    <div>
                                        {efforts.into_iter().map(|level| {
                                            let checked = cur_effort == level;
                                            let level_click = level.clone();
                                            let all_for_key = all_efforts.clone();
                                            let label = effort_label(&level);
                                            view! {
                                                <button type="button" role="radio" aria-checked=checked.to_string()
                                                    on:click=move |_| { on_effort.try_run(level_click.clone()); }
                                                    on:keydown=move |e| {
                                                        if e.key() == "ArrowRight" || e.key() == "ArrowLeft" {
                                                            e.prevent_default();
                                                            let cur = effort.get_untracked();
                                                            let pos = all_for_key.iter().position(|x| x == &cur).unwrap_or(0);
                                                            let len = all_for_key.len();
                                                            let next_idx = if e.key() == "ArrowRight" { (pos + 1) % len } else { (pos + len - 1) % len };
                                                            let next = all_for_key[next_idx].clone();
                                                            { on_effort.try_run(next); };
                                                            if let Some(target) = e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                                                                && let Some(parent) = target.parent_element()
                                                                && let Some(child) = parent.children().item(next_idx as u32)
                                                                && let Ok(html_el) = child.dyn_into::<web_sys::HtmlElement>()
                                                            {
                                                                let _ = html_el.focus();
                                                            }
                                                        }
                                                    }>
                                                    {label}
                                                </button>
                                            }
                                        }).collect_view()}
                                    </div>
                                </div>
                            }
                        })}
                    </div>
                }
            })}
        </div>
    }
}

fn use_model_choice(
    provider: Signal<Option<AgentProvider>>,
    storage_key: String,
) -> (RwSignal<String>, RwSignal<String>, Memo<String>) {
    let model = RwSignal::new(
        storage_get(session_storage(), &format!("{storage_key}.model")).unwrap_or_default(),
    );
    let effort = RwSignal::new(
        storage_get(session_storage(), &format!("{storage_key}.effort")).unwrap_or_default(),
    );
    let key_stored = StoredValue::new(storage_key);

    Effect::new(move |_| {
        if let Some(p) = provider.get()
            && !p.models.is_empty()
        {
            let cur = model.get_untracked();
            if !p.models.iter().any(|m| m.id == cur) {
                let default_id = p
                    .models
                    .iter()
                    .find(|m| m.is_default)
                    .unwrap_or(&p.models[0])
                    .id
                    .clone();
                model.set(default_id);
            }
        }
    });

    Effect::new(move |_| {
        let cur_model = model.get();
        let selected = provider.with(|p| {
            p.as_ref()
                .and_then(|p| p.models.iter().find(|m| m.id == cur_model).cloned())
        });
        if let Some(sel) = selected {
            let cur_eff = effort.get_untracked();
            let valid = sel
                .efforts
                .as_ref()
                .is_some_and(|list| list.contains(&cur_eff));
            if !valid {
                effort.set(sel.default_effort.unwrap_or_default());
            }
        }
    });

    Effect::new(move |_| {
        let m = model.get();
        let e = effort.get();
        let k = key_stored.get_value();
        if !m.is_empty() {
            let _ = storage_set(session_storage(), &format!("{k}.model"), &m);
        }
        if !e.is_empty() {
            let _ = storage_set(session_storage(), &format!("{k}.effort"), &e);
        } else {
            storage_remove(session_storage(), &format!("{k}.effort"));
        }
    });

    let effective_effort = Memo::new(move |_| {
        let m = model.get();
        let e = effort.get();
        let supported = provider.with(|p| {
            p.as_ref()
                .and_then(|p| p.models.iter().find(|item| item.id == m))
                .and_then(|item| item.efforts.as_ref())
                .is_some_and(|list| list.contains(&e))
        });
        if supported { e } else { String::new() }
    });

    (model, effort, effective_effort)
}

// ---------------------------------------------------------------------------
// StepEntry, WorkLog, EarlierRun, WorkLogOpen
// ---------------------------------------------------------------------------

#[component]
fn StepEntry(step: AgentStep) -> impl IntoView {
    let open = RwSignal::new(false);
    let failed = step.status == "failed";
    let running = step.status == "running";

    match step.kind.as_str() {
        "reasoning" => view! {
            <li class="agent-step agent-step-reasoning"><Icon name="agent"/><span><em>{step.label}</em></span></li>
        }.into_any(),
        "plan" => {
            let items = step.items.unwrap_or_default();
            view! {
                <li class="agent-step">
                    <Icon name="list"/>
                    <span>
                        <span class="agent-step-title">"Plan"</span>
                        <ul class="agent-plan">
                            {items.into_iter().map(|item| view! {
                                <li data-done=item.done.then_some("true")>
                                    <span aria-hidden="true">{if item.done { "\u{2713}" } else { "\u{25cb}" }}</span>
                                    <span class="sr-only">{if item.done { "Done: " } else { "To do: " }}</span>
                                    <span>{item.text}</span>
                                </li>
                            }).collect_view()}
                        </ul>
                    </span>
                </li>
            }.into_any()
        }
        "edit" => {
            let verb = if failed { "Failed to edit" } else if running { "Editing" } else { "Edited" };
            let files: Vec<String> = step.label.split('\n').map(str::to_owned).collect();
            view! {
                <li class="agent-step">
                    <Icon name="pencil"/>
                    <span>
                        {verb}" "
                        {files.into_iter().map(|f| view! { <code class="agent-step-file">{f}</code> }).collect_view()}
                    </span>
                </li>
            }.into_any()
        }
        "question" => {
            let items = step.items.unwrap_or_default();
            view! {
                <li class="agent-step">
                    <Icon name="info"/>
                    <span>
                        "Asked "<em>{step.label}</em>
                        {items.into_iter().map(|item| view! { <span class="agent-step-answer">{item.text}</span> }).collect_view()}
                    </span>
                </li>
            }.into_any()
        }
        "tool" if failed && step.label.starts_with("Declined:") => view! {
            <li class="agent-step is-failed">
                <Icon name="lock"/>
                <span>{step.label}" "<small class="muted">"\u{00b7} Dar keeps commands inside the sandbox"</small></span>
            </li>
        }.into_any(),
        "command" => {
            let has_output = step.output.as_ref().is_some_and(|o| !o.is_empty());
            let output_text = step.output.clone().unwrap_or_default();
            let class = if failed { "agent-step is-failed" } else { "agent-step" };
            let exit_code = step.exit_code.filter(|&c| c != 0);
            view! {
                <li class=class>
                    <Icon name="terminal"/>
                    <span>
                        <button type="button" class="agent-step-command" aria-expanded=move || open.get().to_string() disabled=!has_output on:click=move |_| open.update(|v| *v = !*v)>
                            {if running { "Running" } else { "Ran" }}" "<code>{step.label}</code>
                            {exit_code.map(|code| view! { <span class="agent-step-exit">{format!("exit {code}")}</span> })}
                            {running.then(|| view! { <span class="agent-pulse" aria-hidden="true"/> })}
                        </button>
                        {move || (open.get() && has_output).then(|| view! { <pre class="agent-step-output">{output_text.clone()}</pre> })}
                    </span>
                </li>
            }.into_any()
        }
        _ => {
            let class = if failed { "agent-step is-failed" } else { "agent-step" };
            let is_search = step.kind == "search";
            view! {
                <li class=class>
                    <Icon name=if is_search { "search" } else { "plug" }/>
                    <span>{if is_search { "Searched" } else { "Called" }}" "<code>{step.label}</code></span>
                </li>
            }.into_any()
        }
    }
}

#[component]
fn WorkLog(
    #[prop(into)] run: Signal<AgentRun>,
    on_review: Callback<()>,
    #[prop(optional)] on_open_file: Option<Callback<String>>,
) -> impl IntoView {
    let running = Memo::new(move |_| run.with(|r| active_agent_run(&r.status)));
    let now = use_now(running.into(), 1000);
    let open = RwSignal::new(running.get_untracked());
    Effect::new(move |_| {
        open.set(running.get());
    });

    view! {
        {move || {
            let r = run.get();
            let is_running = active_agent_run(&r.status);
            let spent = duration((if is_running { now.get() } else { r.modified_at }) - r.created_at);
            let label = if is_running {
                if r.status == "cancelling" { format!("Stopping \u{00b7} {spent}") } else { format!("Working for {spent}") }
            } else {
                match r.status.as_str() {
                    "completed" => format!("Worked for {spent}"),
                    "cancelled" => format!("Stopped after {spent}"),
                    "failed" => format!("Failed after {spent}"),
                    _ => format!("Interrupted after {spent}"),
                }
            };
            let count = r.document.steps.as_ref().map(Vec::len).unwrap_or(r.document.activity.len());
            let usage = r.document.usage.clone();
            let changes = r.document.changes.clone();
            let (total_added, total_removed) = changes.iter().fold((0usize, 0usize), |(a, rem), ch| {
                let (da, dr) = diff_counts(&ch.diff);
                (a + da, rem + dr)
            });
            let is_open = open.get();
            view! {
                <section class=if is_running { "agent-work is-running" } else { "agent-work" } aria-label="Work log">
                    <span class="sr-only" role="status">{format!("{} \u{00b7} {}", provider_name(&r.model), r.status)}</span>
                    <button type="button" class="agent-work-toggle" aria-expanded=is_open.to_string() on:click=move |_| open.update(|v| *v = !*v)>
                        {is_running.then(|| view! { <span class="agent-pulse" aria-hidden="true"/> })}
                        <span>{label}</span>
                        {(!is_running && count > 0).then(|| view! {
                            <span class="agent-work-count">{format!("\u{00b7} {count} {}", if count == 1 { "step" } else { "steps" })}</span>
                        })}
                        {usage.filter(|_| !is_running).map(|u| view! {
                            <span class="agent-work-count" title=format!("{} input tokens ({} cached), {} output tokens", u.input as i64, u.cached as i64, u.output as i64)>
                                {format!("\u{00b7} {} tokens", tokens(u.input + u.output))}
                            </span>
                        })}
                        {(count > 0 || !is_running).then(|| view! { <Icon name="chevron"/> })}
                    </button>
                    {is_open.then(|| {
                        let steps = r.document.steps.clone();
                        let activity = r.document.activity.clone();
                        view! {
                            <ol class="agent-work-entries">
                                {if let Some(steps) = steps {
                                    steps.into_iter().map(|step| view! { <StepEntry step=step/> }).collect_view().into_any()
                                } else {
                                    activity.into_iter().map(|item| {
                                        if item == "Editing workspace files" {
                                            view! { <li><Icon name="pencil"/><span>"Edited workspace files"</span></li> }.into_any()
                                        } else {
                                            view! { <li><Icon name="terminal"/><span>"Ran "<code>{item}</code></span></li> }.into_any()
                                        }
                                    }).collect_view().into_any()
                                }}
                                {(count == 0).then(|| view! {
                                    <li class="muted"><Icon name="info"/><span>{if is_running { "Waiting for the first step\u{2026}" } else { "No commands or edits were recorded." }}</span></li>
                                })}
                            </ol>
                        }
                    })}
                    {r.document.truncated.then(|| view! { <p class="agent-work-note" role="status">"Output is truncated."</p> })}
                    {(!r.document.error.is_empty()).then(|| view! { <p role="alert" class="error agent-work-note">{r.document.error.clone()}</p> })}
                    {(!is_running && !changes.is_empty()).then(|| {
                        let ch_len = changes.len();
                        view! {
                            <div class="agent-changed-card">
                                <div class="agent-changed-heading">
                                    <strong>{format!("{ch_len} {} changed", if ch_len == 1 { "file" } else { "files" })}</strong>
                                    <DiffStat added=total_added removed=total_removed/>
                                    <button type="button" class="text-button" on:click=move |_| { on_review.try_run(()); }>"Review changes"</button>
                                </div>
                                <ul>
                                    {changes.into_iter().map(|change| {
                                        let (a, rem) = diff_counts(&change.diff);
                                        let icon = match change.kind.as_str() {
                                            "deleted" => "trash",
                                            "added" => "plus",
                                            _ => "file",
                                        };
                                        let path_open = change.path.clone();
                                        view! {
                                            <li>
                                                <Icon name=icon/>
                                                <button type="button" class="agent-changed-file-link" title=change.path.clone() on:click=move |_| {
                                                    if let Some(cb) = on_open_file {
                                                        cb.try_run(path_open.clone());
                                                    } else {
                                                        on_review.try_run(());
                                                    }
                                                }>{change.path.clone()}</button>
                                                <DiffStat added=a removed=rem/>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            </div>
                        }
                    })}
                </section>
            }
        }}
    }
}

#[component]
fn EarlierRun(summary: AgentRunSummary) -> impl IntoView {
    let run: RwSignal<Option<AgentRun>> = RwSignal::new(None);
    let open = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let summary_id = StoredValue::new(summary.id.clone());
    let spent = duration(summary.modified_at - summary.created_at);
    let label = match summary.status.as_str() {
        "completed" => format!("Worked for {spent}"),
        "cancelled" => format!("Stopped after {spent}"),
        "failed" => format!("Failed after {spent}"),
        _ => format!("Interrupted after {spent}"),
    };
    let label = StoredValue::new(label);

    Effect::new(move |_| {
        if !open.get() || run.with(Option::is_some) {
            return;
        }
        let controller = web_sys::AbortController::new().expect("AbortController");
        let signal = controller.signal();
        let ctrl_keep = SendWrapper::new(controller);
        on_cleanup(move || ctrl_keep.abort());
        let id = summary_id.get_value();
        spawn_local(async move {
            match api::request::<AgentRun>(
                &format!("{BASE}/runs/{id}"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(val) => {
                    if !signal.aborted() {
                        run.try_set(Some(val));
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        error.try_set(failure(&e));
                    }
                }
            }
        });
    });

    view! {
        {move || {
            if open.get() && let Some(loaded) = run.get() {
                view! { <div class="agent-earlier-run"><WorkLogOpen run=loaded on_close=Callback::new(move |_| open.set(false))/></div> }.into_any()
            } else {
                let err = error.get();
                let loading = open.get() && run.with(Option::is_none) && err.is_empty();
                view! {
                    <section class="agent-work" aria-label="Earlier work log">
                        <button type="button" class="agent-work-toggle" aria-expanded="false" on:click=move |_| open.set(true)>
                            <span>{label.get_value()}</span>
                            <Icon name="chevron"/>
                        </button>
                        {loading.then(|| view! { <p class="agent-work-note" role="status">"Loading\u{2026}"</p> })}
                        {(!err.is_empty()).then(|| view! { <p class="error agent-work-note" role="alert">{err}</p> })}
                    </section>
                }.into_any()
            }
        }}
    }
}

#[component]
fn WorkLogOpen(run: AgentRun, on_close: Callback<()>) -> impl IntoView {
    let spent = duration(run.modified_at - run.created_at);
    let verb = if run.status == "completed" {
        "Worked"
    } else {
        "Ran"
    };
    let steps = run.document.steps;
    let activity = run.document.activity;
    let count = steps.as_ref().map(Vec::len).unwrap_or(activity.len());
    let changes_len = run.document.changes.len();

    view! {
        <section class="agent-work" aria-label="Earlier work log">
            <button type="button" class="agent-work-toggle" aria-expanded="true" on:click=move |_| { on_close.try_run(()); }>
                <span>{format!("{verb} for {spent}")}</span>
                <Icon name="chevron"/>
            </button>
            <ol class="agent-work-entries">
                {if let Some(steps) = steps {
                    steps.into_iter().map(|step| view! { <StepEntry step=step/> }).collect_view().into_any()
                } else {
                    activity.into_iter().map(|item| view! { <li><Icon name="terminal"/><span>"Ran "<code>{item}</code></span></li> }).collect_view().into_any()
                }}
                {(count == 0).then(|| view! { <li class="muted"><Icon name="info"/><span>"No commands or edits were recorded."</span></li> })}
            </ol>
            {(changes_len > 0).then(|| view! {
                <p class="agent-work-note">{format!("{changes_len} {} changed in this turn.", if changes_len == 1 { "file" } else { "files" })}</p>
            })}
        </section>
    }
}

// ---------------------------------------------------------------------------
// Pictures, PictureTray, AttachButton
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Picture {
    id: String,
    file: SendWrapper<web_sys::File>,
    name: String,
    url: String,
    status: &'static str, // "uploading" | "ready" | "failed"
    error: Option<String>,
}

#[derive(Clone, Copy)]
struct PicturesState {
    items: RwSignal<Vec<Picture>>,
    thread_id: StoredValue<Option<String>>,
    csrf: StoredValue<String>,
    controller: StoredValue<web_sys::AbortController, LocalStorage>,
}

impl PicturesState {
    fn new(thread_id: Option<String>, csrf: String) -> Self {
        let items = RwSignal::new(Vec::<Picture>::new());
        let controller =
            StoredValue::new_local(web_sys::AbortController::new().expect("AbortController"));
        on_cleanup(move || {
            controller.with_value(|c| c.abort());
            items.with_untracked(|list| {
                for item in list {
                    let _ = web_sys::Url::revoke_object_url(&item.url);
                }
            });
        });
        Self {
            items,
            thread_id: StoredValue::new(thread_id),
            csrf: StoredValue::new(csrf),
            controller,
        }
    }

    fn upload(self, item: &Picture, thread: String) {
        let safe_name: String = item
            .name
            .chars()
            .map(|c| {
                if (c as u32) < 0x20 || c == '/' || c == '\\' {
                    '_'
                } else {
                    c
                }
            })
            .take(200)
            .collect();
        let safe_name = if safe_name.is_empty() {
            "image".to_owned()
        } else {
            safe_name
        };
        let url = format!(
            "{BASE}/threads/{thread}/attachments/{}?name={}",
            item.id,
            encode_uri(&safe_name)
        );
        let content_type = item.file.type_();
        let raw_file: JsValue = item.file.clone().take().into();
        let id = item.id.clone();
        let signal = self.controller.with_value(|c| c.signal());
        let token = self.csrf.get_value();
        spawn_local(async move {
            let res = api::request::<AgentAttachment>(
                &url,
                Req {
                    method: "PUT",
                    raw: Some(raw_file),
                    csrf: Some(&token),
                    headers: &[("content-type", &content_type)],
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await;
            if signal.aborted() {
                return;
            }
            match res {
                Ok(_) => {
                    self.items.try_update(|list| {
                        if let Some(row) = list.iter_mut().find(|r| r.id == id) {
                            row.status = "ready";
                        }
                    });
                }
                Err(e) => {
                    let msg = failure(&e);
                    self.items.try_update(|list| {
                        if let Some(row) = list.iter_mut().find(|r| r.id == id) {
                            row.status = "failed";
                            row.error = Some(msg);
                        }
                    });
                }
            }
        });
    }

    fn add(self, files: Vec<web_sys::File>) -> String {
        let current_len = self.items.with_untracked(Vec::len);
        let remaining = AGENT_ATTACHMENT_MAX_COUNT.saturating_sub(current_len);
        let accepted: Vec<web_sys::File> = files
            .iter()
            .filter(|f| {
                AGENT_IMAGE_TYPES.contains(&f.type_().as_str())
                    && (f.size() as u64) <= AGENT_ATTACHMENT_MAX_BYTES
            })
            .take(remaining)
            .cloned()
            .collect();
        let thread = self.thread_id.get_value();
        let rows: Vec<Picture> = accepted
            .iter()
            .map(|f| {
                let url = web_sys::Url::create_object_url_with_blob(f).unwrap_or_default();
                Picture {
                    id: random_uuid(),
                    name: f.name(),
                    file: SendWrapper::new(f.clone()),
                    url,
                    status: if thread.is_some() {
                        "uploading"
                    } else {
                        "ready"
                    },
                    error: None,
                }
            })
            .collect();
        self.items.update(|list| list.extend(rows.iter().cloned()));
        if let Some(tid) = thread {
            for row in &rows {
                self.upload(row, tid.clone());
            }
        }
        if accepted.len() == files.len() {
            String::new()
        } else {
            format!(
                "Only PNG, JPEG, GIF or WebP images up to 8 MiB, at most {AGENT_ATTACHMENT_MAX_COUNT} per message."
            )
        }
    }

    fn remove(self, id: &str) {
        self.items.update(|list| {
            if let Some(pos) = list.iter().position(|item| item.id == id) {
                let removed = list.remove(pos);
                let _ = web_sys::Url::revoke_object_url(&removed.url);
            }
        });
    }

    fn clear(self) {
        self.items.update(|list| {
            for item in list.drain(..) {
                let _ = web_sys::Url::revoke_object_url(&item.url);
            }
        });
    }

    fn busy(self) -> bool {
        self.items
            .with(|list| list.iter().any(|item| item.status == "uploading"))
    }

    fn failed(self) -> bool {
        self.items
            .with(|list| list.iter().any(|item| item.status == "failed"))
    }
}

#[component]
fn PictureTray(pictures: PicturesState) -> impl IntoView {
    view! {
        {move || {
            let list = pictures.items.get();
            (!list.is_empty()).then(|| view! {
                <ul class="agent-pictures" aria-label="Attached images">
                    {list.into_iter().map(|item| {
                        let id = item.id.clone();
                        view! {
                            <li data-status=item.status>
                                <img src=item.url alt=item.name.clone()/>
                                {(item.status == "uploading").then(|| view! { <span class="agent-picture-state" role="status">"Uploading\u{2026}"</span> })}
                                {(item.status == "failed").then(|| view! { <span class="agent-picture-state error" role="alert" title=item.error.clone()>"Failed"</span> })}
                                <button type="button" class="icon-button" aria-label=format!("Remove {}", item.name) on:click=move |_| pictures.remove(&id)><Icon name="close"/></button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            })
        }}
    }
}

#[component]
fn AttachButton(
    on_files: Callback<Vec<web_sys::File>>,
    #[prop(into)] disabled: Signal<bool>,
) -> impl IntoView {
    let input = NodeRef::<html::Input>::new();
    let accept = AGENT_IMAGE_TYPES.join(",");
    view! {
        <button type="button" class="icon-button agent-attach" aria-label="Attach images" title="Attach images" disabled=move || disabled.get()
            on:click=move |_| if let Some(el) = input.get_untracked() { el.click(); }>
            <Icon name="plus"/>
        </button>
        <input node_ref=input type="file" accept=accept multiple hidden
            on:change=move |e| {
                let target: web_sys::HtmlInputElement = event_target(&e);
                let files = files_from_list(target.files(), false);
                { on_files.try_run(files); };
                target.set_value("");
            }/>
    }
}

// ---------------------------------------------------------------------------
// ContextMeter, QuestionCard, Minimap
// ---------------------------------------------------------------------------

#[component]
fn ContextMeter(
    #[prop(into)] used: Signal<Option<f64>>,
    #[prop(into)] window_size: Signal<Option<f64>>,
) -> impl IntoView {
    view! {
        {move || {
            let u = used.get().filter(|&v| v > 0.0)?;
            let w = window_size.get().filter(|&v| v > 0.0)?;
            let share = (u / w).min(1.0);
            let percent = (share * 100.0).round() as i64;
            let level = if share > 0.85 { Some("high") } else if share > 0.6 { Some("medium") } else { None };
            Some(view! {
                <span class="agent-context-meter" role="img"
                    aria-label=format!("Context {percent}% used: {} of {} tokens", tokens(u), tokens(w))
                    title=format!("{} of {} tokens in the last request", tokens(u), tokens(w))
                    data-level=level>
                    <svg viewBox="0 0 20 20" aria-hidden="true">
                        <circle cx="10" cy="10" r="8"/>
                        <circle cx="10" cy="10" r="8" pathLength="100" stroke-dasharray=format!("{percent} 100")/>
                    </svg>
                    <span>{format!("{percent}%")}</span>
                </span>
            })
        }}
    }
}

#[component]
fn QuestionCard(#[prop(into)] run: Signal<AgentRun>, session: BrowserSession) -> impl IntoView {
    let answers: RwSignal<HashMap<String, String>> = RwSignal::new(HashMap::new());
    let other: RwSignal<HashMap<String, String>> = RwSignal::new(HashMap::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let csrf = StoredValue::new(session.csrf_token);
    let last_pending_id = StoredValue::new_local(RefCell::new(String::new()));

    Effect::new(move |_| {
        let pid = run.with(|r| {
            r.document
                .pending
                .as_ref()
                .map(|p| p.id.clone())
                .unwrap_or_default()
        });
        let changed = last_pending_id.with_value(|c| {
            let mut cur = c.borrow_mut();
            if *cur != pid {
                *cur = pid;
                true
            } else {
                false
            }
        });
        if changed {
            answers.set(HashMap::new());
            other.set(HashMap::new());
            error.set(String::new());
            busy.set(false);
        }
    });

    let answer_value = move |qid: &str| -> String {
        let chosen = answers.with(|m| m.get(qid).cloned().unwrap_or_default());
        if chosen == "\u{0000}other" {
            other.with(|m| m.get(qid).map(|s| s.trim().to_owned()).unwrap_or_default())
        } else {
            chosen
        }
    };

    let ready = Memo::new(move |_| {
        run.with(|r| {
            let Some(pending) = r.document.pending.as_ref() else {
                return false;
            };
            pending
                .questions
                .iter()
                .all(|q| !answer_value(&q.id).is_empty())
        })
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        if !ready.get_untracked() || busy.get_untracked() {
            return;
        }
        let r = run.get_untracked();
        let Some(pending) = r.document.pending else {
            return;
        };
        busy.set(true);
        error.set(String::new());
        let mut ans_map = serde_json::Map::new();
        for q in &pending.questions {
            ans_map.insert(q.id.clone(), json!([answer_value(&q.id)]));
        }
        let body = json!({ "requestId": pending.id, "answers": ans_map }).to_string();
        let run_id = r.id;
        let token = csrf.get_value();
        spawn_local(async move {
            if let Err(e) = api::request::<Value>(
                &format!("{BASE}/runs/{run_id}/answer"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    ..Default::default()
                },
            )
            .await
            {
                error.try_set(failure(&e));
                busy.try_set(false);
            }
        });
    };

    view! {
        {move || {
            let r = run.get();
            let pending = r.document.pending?;
            let prov = provider_name(&r.model);
            Some(view! {
                <form class="agent-question" aria-label=format!("{prov} is asking") on:submit=submit>
                    <div class="agent-question-title"><Icon name="info"/><strong>{format!("{prov} needs your input")}</strong></div>
                    {pending.questions.into_iter().map(|q| {
                        let qid = q.id.clone();
                        let has_options = !q.options.is_empty();
                        let show_other = q.other || !has_options;
                        let qid_other_radio = q.id.clone();
                        let qid_other_focus = q.id.clone();
                        let qid_other_input = q.id.clone();
                        let qid_other_val = q.id.clone();
                        let qid_other_chk = q.id.clone();
                        let q_text = q.question.clone();
                        view! {
                            <fieldset>
                                <legend>
                                    {(!q.header.is_empty()).then(|| view! { <small>{q.header}</small> })}
                                    <span>{q.question}</span>
                                </legend>
                                {q.options.into_iter().enumerate().map(|(opt_idx, opt)| {
                                    let qid_chk = qid.clone();
                                    let qid_chk_cls = qid.clone();
                                    let qid_chg = qid.clone();
                                    let lbl_chk = opt.label.clone();
                                    let lbl_chk_cls = opt.label.clone();
                                    let lbl_chg = opt.label.clone();
                                    view! {
                                        <label class=move || if answers.with(|m| m.get(&qid_chk_cls).is_some_and(|v| v == &lbl_chk_cls)) { "agent-q-option is-selected" } else { "agent-q-option" }>
                                            <input type="radio" class="agent-q-radio" name=format!("q-{qid}")
                                                prop:checked=move || answers.with(|m| m.get(&qid_chk).is_some_and(|v| v == &lbl_chk))
                                                on:change=move |_| { answers.update(|m| { m.insert(qid_chg.clone(), lbl_chg.clone()); }); }/>
                                            <span class="agent-q-key" aria-hidden="true">{opt_idx + 1}</span>
                                            <span class="agent-q-copy">
                                                <strong>{opt.label}</strong>
                                                {(!opt.description.is_empty()).then(|| view! { <small>{opt.description}</small> })}
                                            </span>
                                        </label>
                                    }
                                }).collect_view()}
                                {show_other.then(|| view! {
                                    <label class="agent-q-option agent-question-other">
                                        {has_options.then(|| {
                                            let name = format!("q-{qid_other_radio}");
                                            let qid_r_chg = qid_other_radio.clone();
                                            view! {
                                                <input type="radio" class="agent-q-radio" name=name
                                                    prop:checked=move || answers.with(|m| m.get(&qid_other_chk).is_some_and(|v| v == "\u{0000}other"))
                                                    on:change=move |_| { answers.update(|m| { m.insert(qid_r_chg.clone(), "\u{0000}other".into()); }); }/>
                                            }
                                        })}
                                        <input type=if q.secret { "password" } else { "text" }
                                            aria-label=if has_options { "Other answer".to_owned() } else { q_text }
                                            placeholder=if has_options { "Other\u{2026}" } else { "Your answer" }
                                            prop:value=move || other.with(|m| m.get(&qid_other_val).cloned().unwrap_or_default())
                                            maxlength="2000"
                                            on:focus=move |_| { answers.update(|m| { m.insert(qid_other_focus.clone(), "\u{0000}other".into()); }); }
                                            on:input=move |e| { let v = event_target_value(&e); other.update(|m| { m.insert(qid_other_input.clone(), v); }); }/>
                                    </label>
                                })}
                            </fieldset>
                        }
                    }).collect_view()}
                    {move || { let e = error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
                    <div class="agent-question-actions">
                        <button class="primary-button" disabled=move || !ready.get() || busy.get()>
                            {move || if busy.get() { "Sending\u{2026}" } else { "Send answer" }}
                        </button>
                    </div>
                </form>
            })
        }}
    }
}

#[derive(Clone, Debug, PartialEq)]
struct MinimapMark {
    top: f64,
    height: f64,
    role: &'static str,
    id: String,
}

#[component]
fn Minimap(
    viewport: NodeRef<html::Div>,
    content: NodeRef<html::Div>,
    #[prop(into)] version: Signal<String>,
) -> impl IntoView {
    let marks: RwSignal<Vec<MinimapMark>> = RwSignal::new(Vec::new());
    let view_range = RwSignal::new((0.0f64, 1.0f64));
    let rail = NodeRef::<html::Div>::new();

    Effect::new(move |_| {
        let _ver = version.get();
        let (Some(vp), Some(node)) = (viewport.get(), content.get()) else {
            return;
        };
        let measure = {
            let vp = vp.clone();
            let node = node.clone();
            move || {
                let total = (vp.scroll_height() as f64).max(1.0);
                let mut next_marks = Vec::new();
                if let Ok(list) = node.query_selector_all("[data-message-id]") {
                    for i in 0..list.length() {
                        if let Some(el) = list
                            .item(i)
                            .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
                        {
                            let top = (el.offset_top() as f64) / total;
                            let height = ((el.offset_height() as f64) / total).max(0.004);
                            let role = if el.class_list().contains("agent-message-user") {
                                "user"
                            } else {
                                "assistant"
                            };
                            let id = el.get_attribute("data-message-id").unwrap_or_default();
                            next_marks.push(MinimapMark {
                                top,
                                height,
                                role,
                                id,
                            });
                        }
                    }
                }
                marks.try_set(next_marks);
                view_range.try_set((
                    (vp.scroll_top() as f64) / total,
                    ((vp.client_height() as f64) / total).min(1.0),
                ));
            }
        };
        measure();
        let cb_obs = {
            let m = measure.clone();
            Closure::<dyn FnMut()>::new(move || m())
        };
        let observer =
            web_sys::ResizeObserver::new(cb_obs.as_ref().unchecked_ref()).expect("ResizeObserver");
        observer.observe(&node);
        observer.observe(&vp);
        let scroll_listener = Listener::new(&vp, "scroll", false, move |_| measure());
        let kept = SendWrapper::new((observer, cb_obs, scroll_listener));
        on_cleanup(move || {
            let (observer, _cb, _scroll) = kept.take();
            observer.disconnect();
        });
    });

    Effect::new(move |_| {
        let list = marks.get();
        let (vtop, vheight) = view_range.get();
        let Some(node) = rail.get() else { return };
        if let Ok(items) = node.query_selector_all(".agent-minimap-mark") {
            for (index, mark) in list.iter().enumerate() {
                if let Some(el) = items
                    .item(index as u32)
                    .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = el.style().set_property("--top", &mark.top.to_string());
                    let _ = el
                        .style()
                        .set_property("--height", &mark.height.to_string());
                }
            }
        }
        let _ = web_sys::HtmlElement::style(&node).set_property("--view-top", &vtop.to_string());
        let _ =
            web_sys::HtmlElement::style(&node).set_property("--view-height", &vheight.to_string());
    });

    view! {
        {move || {
            let list = marks.get();
            let (_, vh) = view_range.get();
            (list.len() >= 4 && vh < 1.0).then(|| view! {
                <div class="agent-minimap" node_ref=rail aria-hidden="true"
                    on:pointerdown=move |e| {
                        if let Some(r) = rail.get_untracked()
                            && let Some(vp) = viewport.get_untracked()
                        {
                            let rect = r.get_bounding_client_rect();
                            let fraction = ((e.client_y() as f64) - rect.top()) / rect.height().max(1.0);
                            let target = (fraction * (vp.scroll_height() as f64) - (vp.client_height() as f64) / 2.0) as i32;
                            vp.set_scroll_top(target);
                        }
                    }>
                    {list.into_iter().map(|mark| view! {
                        <span data-role=mark.role class="agent-minimap-mark"/>
                    }).collect_view()}
                    <span class="agent-minimap-view"/>
                </div>
            })
        }}
    }
}

// ---------------------------------------------------------------------------
// PromptInput
// ---------------------------------------------------------------------------

fn parse_slash(value: &str) -> Option<(String, Option<String>)> {
    let rest = value.strip_prefix('/')?;
    if let Some((cmd, arg)) = rest.split_once(' ') {
        if cmd.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
            && arg.bytes().all(|b| b.is_ascii_lowercase())
        {
            return Some((cmd.to_owned(), Some(arg.to_owned())));
        }
        None
    } else if rest.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
        Some((rest.to_owned(), None))
    } else {
        None
    }
}

fn parse_mention(before: &str) -> Option<String> {
    let idx = before.rfind('@')?;
    if idx > 0 {
        let prev = before[..idx].chars().next_back()?;
        if !prev.is_whitespace() {
            return None;
        }
    }
    let query = &before[idx + 1..];
    if query.chars().any(|c| c.is_whitespace() || c == '@') {
        return None;
    }
    Some(query.to_owned())
}

fn slice_utf16(s: &str, utf16_end: usize) -> &str {
    let mut count = 0;
    for (byte_idx, ch) in s.char_indices() {
        if count >= utf16_end {
            return &s[..byte_idx];
        }
        count += ch.len_utf16();
    }
    s
}

#[derive(Clone)]
struct Suggestion {
    label: String,
    detail: Option<String>,
    apply: Callback<()>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct ThreadFilesResult {
    source: String,
    items: Vec<String>,
}

#[component]
fn PromptInput(
    input_ref: NodeRef<html::Textarea>,
    #[prop(into)] value: Signal<String>,
    on_change: Callback<String>,
    on_submit: Callback<()>,
    #[prop(into)] disabled: Signal<bool>,
    #[prop(into)] placeholder: Signal<String>,
    #[prop(into)] send_key: Signal<&'static str>,
    #[prop(optional, into)] thread_id: Option<Signal<String>>,
    #[prop(into)] prompts: Signal<Vec<String>>,
    #[prop(into)] commands: Signal<Vec<SlashCommand>>,
    #[prop(optional)] on_files: Option<Callback<Vec<web_sys::File>>>,
) -> impl IntoView {
    let caret = RwSignal::new(0usize);
    let active = RwSignal::new(0usize);
    let files: RwSignal<Option<(String, ThreadFilesResult)>> = RwSignal::new(None);
    let dismissed = RwSignal::new(String::new());
    let history_index = StoredValue::new_local(RefCell::new(-1i32));
    let stash = StoredValue::new_local(RefCell::new(String::new()));

    let mention_query = Memo::new(move |_| {
        let v = value.get();
        let before = slice_utf16(&v, caret.get());
        parse_mention(before)
    });

    Effect::new(move |_| {
        let mq = mention_query.get();
        let tid = thread_id.map(|t| t.get()).filter(|s| !s.is_empty());
        let (Some(query), Some(tid)) = (mq, tid) else {
            files.set(None);
            return;
        };
        let controller = web_sys::AbortController::new().expect("AbortController");
        let signal = controller.signal();
        let timer = set_timeout(120, move || {
            let url = format!("{BASE}/threads/{tid}/files?query={}", encode_uri(&query));
            spawn_local(async move {
                if let Ok(res) = api::request::<ThreadFilesResult>(
                    &url,
                    Req {
                        signal: Some(signal.clone()),
                        ..Default::default()
                    },
                )
                .await
                    && !signal.aborted()
                {
                    files.try_set(Some((query, res)));
                }
            });
        });
        let kept = SendWrapper::new((controller, RefCell::new(timer)));
        on_cleanup(move || {
            let (ctrl, timer_cell) = kept.take();
            clear_timeout(&mut timer_cell.borrow_mut());
            ctrl.abort();
        });
    });

    let compute = move || -> (String, String, Vec<Suggestion>, bool) {
        let val = value.get();
        let c = caret.get();
        let before = slice_utf16(&val, c);
        let slash = parse_slash(&val);
        let mention = parse_mention(before);
        let trigger = if slash.is_some() {
            format!("slash:{val}")
        } else if let Some(ref m) = mention {
            format!("mention:{m}:{c}")
        } else {
            String::new()
        };
        let is_dismissed = dismissed.get() == trigger;
        let mut suggestions = Vec::new();
        let mut heading = String::new();
        if let Some((ref cmd_name, ref arg_opt)) = slash
            && !is_dismissed
        {
            let cmds = commands.get();
            if let Some(cmd) = cmds.iter().find(|item| &item.name == cmd_name)
                && let Some(ref args) = cmd.argument
                && let Some(arg_val) = arg_opt
            {
                heading = format!("/{}", cmd.name);
                for item in args.iter().filter(|a| a.starts_with(arg_val.as_str())) {
                    let run_cb = cmd.run;
                    let arg_s = item.clone();
                    suggestions.push(Suggestion {
                        label: effort_label(item),
                        detail: None,
                        apply: Callback::new(move |_| {
                            run_cb.run(arg_s.clone());
                            {
                                on_change.try_run(String::new());
                            };
                        }),
                    });
                }
            } else {
                heading = "Commands".into();
                for item in cmds
                    .into_iter()
                    .filter(|c| c.name.starts_with(cmd_name.as_str()))
                {
                    let name_clone = item.name.clone();
                    let has_arg = item.argument.is_some();
                    let run_cb = item.run;
                    suggestions.push(Suggestion {
                        label: format!("/{}", item.name),
                        detail: Some(item.hint),
                        apply: Callback::new(move |_| {
                            if has_arg {
                                let next = format!("/{name_clone} ");
                                let pos = next.encode_utf16().count();
                                {
                                    on_change.try_run(next);
                                };
                                caret.set(pos);
                            } else {
                                run_cb.run(String::new());
                                {
                                    on_change.try_run(String::new());
                                };
                            }
                        }),
                    });
                }
            }
        } else if let Some(ref m) = mention
            && !is_dismissed
        {
            let has_tid = thread_id.is_some_and(|t| !t.with(String::is_empty));
            let f_state = files.get();
            heading = if !has_tid {
                "Files are available once the thread starts".into()
            } else if f_state.as_ref().is_some_and(|(_, r)| r.source == "none") {
                "Files appear after the first run".into()
            } else {
                "Files".into()
            };
            if let Some((ref fq, ref res)) = f_state
                && fq == m
            {
                for path in res.items.iter().take(12) {
                    let file_label = path.rsplit('/').next().unwrap_or(path).to_owned();
                    let detail = path.contains('/').then(|| path.clone());
                    let path_cl = path.clone();
                    let m_len_u16 = m.encode_utf16().count();
                    suggestions.push(Suggestion {
                        label: file_label,
                        detail,
                        apply: Callback::new(move |_| {
                            let cur_val = value.get_untracked();
                            let cur_caret = caret.get_untracked();
                            let start_u16 = cur_caret.saturating_sub(m_len_u16 + 1);
                            let prefix = slice_utf16(&cur_val, start_u16);
                            let suffix = &cur_val[slice_utf16(&cur_val, cur_caret).len()..];
                            let next = format!("{prefix}@{path_cl} {suffix}");
                            let pos = start_u16 + path_cl.encode_utf16().count() + 2;
                            {
                                on_change.try_run(next);
                            };
                            caret.set(pos);
                            request_animation_frame(move || {
                                if let Some(el) = input_ref.get_untracked() {
                                    let _ = el.set_selection_range(pos as u32, pos as u32);
                                }
                            });
                        }),
                    });
                }
            }
        }
        (trigger, heading, suggestions, slash.is_some())
    };

    let trigger_and_count = Memo::new(move |_| {
        let (trigger, _, suggestions, _) = compute();
        (trigger, suggestions.len())
    });

    Effect::new(move |_| {
        let _ = trigger_and_count.get();
        active.set(0);
    });

    let on_keydown = move |event: web_sys::KeyboardEvent| {
        if event.is_composing() {
            return;
        }
        let (trigger, heading, suggestions, is_slash) = compute();
        let open = !heading.is_empty();
        let key = event.key();
        if open && !suggestions.is_empty() && (key == "ArrowDown" || key == "ArrowUp") {
            event.prevent_default();
            let len = suggestions.len();
            let delta = if key == "ArrowDown" { 1 } else { len - 1 };
            active.update(|idx| *idx = (*idx + delta) % len);
            return;
        }
        if open
            && let Some(item) = suggestions.get(active.get_untracked())
            && (key == "Tab" || (key == "Enter" && !event.shift_key()))
        {
            event.prevent_default();
            item.apply.run(());
            return;
        }
        if open && key == "Escape" {
            event.prevent_default();
            event.stop_propagation();
            dismissed.set(trigger);
            return;
        }
        let field: web_sys::HtmlTextAreaElement = event_target(&event);
        let sel_start = field.selection_start().ok().flatten().unwrap_or(0);
        let sel_end = field.selection_end().ok().flatten().unwrap_or(0);
        let cur_val = value.get_untracked();
        let prompt_list = prompts.get_untracked();
        if key == "ArrowUp"
            && !prompt_list.is_empty()
            && sel_start == 0
            && sel_end == 0
            && (history_index.with_value(|h| *h.borrow() >= 0) || cur_val.is_empty())
        {
            event.prevent_default();
            let idx = history_index.with_value(|h| {
                let mut h = h.borrow_mut();
                if *h < 0 {
                    stash.with_value(|s| *s.borrow_mut() = cur_val.clone());
                }
                *h = (*h + 1).min((prompt_list.len() as i32) - 1);
                *h as usize
            });
            {
                on_change.try_run(prompt_list[prompt_list.len() - 1 - idx].clone());
            };
            return;
        }
        let val_u16_len = cur_val.encode_utf16().count() as u32;
        if key == "ArrowDown"
            && history_index.with_value(|h| *h.borrow() >= 0)
            && sel_start == val_u16_len
        {
            event.prevent_default();
            let next_idx = history_index.with_value(|h| {
                let mut h = h.borrow_mut();
                *h -= 1;
                *h
            });
            if next_idx < 0 {
                {
                    on_change.try_run(stash.with_value(|s| s.borrow().clone()));
                };
            } else {
                {
                    on_change
                        .try_run(prompt_list[prompt_list.len() - 1 - (next_idx as usize)].clone());
                };
            }
            return;
        }
        let modifier = event.ctrl_key() || event.meta_key();
        if key == "Enter"
            && !event.shift_key()
            && !event.alt_key()
            && (send_key.get_untracked() == "enter" || modifier)
        {
            event.prevent_default();
            if is_slash && let Some((cmd_name, arg_opt)) = parse_slash(&cur_val) {
                let cmds = commands.get_untracked();
                let arg_str = arg_opt.unwrap_or_default();
                if let Some(cmd) = cmds.iter().find(|c| c.name == cmd_name)
                    && (cmd.argument.is_none()
                        || cmd.argument.as_ref().is_some_and(|a| a.contains(&arg_str)))
                {
                    cmd.run.run(arg_str);
                    {
                        on_change.try_run(String::new());
                    };
                    return;
                }
            }
            history_index.with_value(|h| *h.borrow_mut() = -1);
            {
                on_submit.try_run(());
            };
        }
    };

    view! {
        <div class="agent-prompt">
            {move || {
                let (_, heading, suggestions, is_slash) = compute();
                let open = !heading.is_empty();
                let cur_active = active.get();
                let has_tid = thread_id.is_some_and(|t| !t.with(String::is_empty));
                let files_none = files.with(|f| f.as_ref().is_some_and(|(_, r)| r.source == "none"));
                open.then(|| view! {
                    <div class="agent-suggestions" role="listbox" id="agent-suggestions" aria-label=heading.clone()>
                        <div class="agent-suggestions-heading">{heading.clone()}</div>
                        {suggestions.iter().enumerate().map(|(index, item)| {
                            let is_act = index == cur_active;
                            let apply = item.apply;
                            let detail = item.detail.clone();
                            view! {
                                <div id=format!("agent-suggestion-{index}") role="option" aria-selected=is_act.to_string() data-active=is_act.then_some("true")
                                    on:pointerdown=move |e| e.prevent_default()
                                    on:pointerenter=move |_| active.set(index)
                                    on:click=move |_| apply.run(())>
                                    <strong>{item.label.clone()}</strong>
                                    {detail.map(|d| view! { <small>{d}</small> })}
                                </div>
                            }
                        }).collect_view()}
                        {suggestions.is_empty().then(|| view! {
                            <div class="agent-suggestions-empty">
                                {if is_slash { "No matching commands" } else if has_tid && !files_none { "No matching files" } else { "" }}
                            </div>
                        })}
                    </div>
                })
            }}
            <label class="sr-only" for="agent-message">"Message"</label>
            <textarea id="agent-message" node_ref=input_ref
                prop:value=move || value.get()
                disabled=move || disabled.get()
                placeholder=move || placeholder.get()
                role="combobox"
                aria-expanded=move || { let (_, h, _, _) = compute(); (!h.is_empty()).to_string() }
                aria-controls=move || { let (_, h, _, _) = compute(); (!h.is_empty()).then_some("agent-suggestions") }
                aria-autocomplete="list"
                aria-activedescendant=move || {
                    let (_, h, suggestions, _) = compute();
                    let cur = active.get();
                    (!h.is_empty() && cur < suggestions.len()).then(|| format!("agent-suggestion-{cur}"))
                }
                on:input=move |e| {
                    history_index.with_value(|h| *h.borrow_mut() = -1);
                    dismissed.set(String::new());
                    let target: web_sys::HtmlTextAreaElement = event_target(&e);
                    { on_change.try_run(target.value()); };
                    caret.set(target.selection_start().ok().flatten().unwrap_or(0) as usize);
                }
                on:select=move |e| {
                    let target: web_sys::HtmlTextAreaElement = event_target(&e);
                    caret.set(target.selection_start().ok().flatten().unwrap_or(0) as usize);
                }
                on:keydown=on_keydown
                on:paste=move |e: web_sys::ClipboardEvent| {
                    if let Some(cb) = on_files
                        && let Some(dt) = e.clipboard_data()
                    {
                        let pasted = files_from_list(dt.files(), true);
                        if !pasted.is_empty() {
                            e.prevent_default();
                            cb.run(pasted);
                        }
                    }
                }/>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Conversation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingOp {
    id: String,
    content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunOp {
    id: String,
    message_id: String,
    content: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct MessageCreated {
    thread: AgentThread,
    message: AgentMessage,
}

#[component]
fn Conversation(
    thread: AgentThread,
    #[prop(into)] project: Signal<Option<AgentProject>>,
    session: BrowserSession,
    on_changed: Callback<AgentThread>,
    draft_cache: StoredValue<RefCell<HashMap<String, String>>, LocalStorage>,
    positions: StoredValue<RefCell<HashMap<String, ScrollPosition>>, LocalStorage>,
    #[prop(into)] provider: Signal<Option<AgentProvider>>,
    #[prop(into)] checking_provider: Signal<bool>,
    #[prop(into)] run: Signal<Option<AgentRun>>,
    on_run: Callback<String>,
    on_stop: Callback<()>,
    on_review: Callback<()>,
    #[prop(into)] history_epoch: Signal<u64>,
    #[prop(into)] find_open: Signal<bool>,
    on_close_find: Callback<()>,
    auto_start: Option<bool>,
    on_started: Callback<()>,
    #[prop(into)] send_key: Signal<&'static str>,
    #[prop(into)] follow_up_mode: Signal<&'static str>,
    #[prop(into)] starred_models: Signal<Vec<String>>,
    on_toggle_star: Callback<String>,
    context_chips: RwSignal<Vec<ContextChip>>,
    #[prop(into)] stash: Signal<Vec<PromptStashEntry>>,
    on_update_stash: Callback<Vec<PromptStashEntry>>,
    on_open_file: Callback<String>,
    incoming_draft: RwSignal<Option<String>>,
    commands: Callback<SlashControls, Vec<SlashCommand>>,
    initial_files: Option<Vec<SendWrapper<web_sys::File>>>,
    on_forked: Callback<AgentThread>,
) -> impl IntoView {
    let thread_id = thread.id.clone();
    let thread_id_sv = StoredValue::new(thread_id.clone());
    let key = format!("dar.agent.draft.{}.{thread_id}", session.user.id);
    let key_sv = StoredValue::new(key.clone());
    let operation_key = StoredValue::new(format!("{key}.operation"));
    let csrf = StoredValue::new(session.csrf_token.clone());
    let session_sv = StoredValue::new(session.clone());

    let initial_draft = draft_cache.with_value(|cache| {
        cache
            .borrow()
            .get(&key)
            .cloned()
            .or_else(|| storage_get(session_storage(), &key))
            .unwrap_or_default()
    });
    let draft = RwSignal::new(initial_draft);
    let history: RwSignal<Option<AgentHistory>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let storage_error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let notice = RwSignal::new(String::new());
    let reload = RwSignal::new(0u64);
    // The history is being reloaded; a send made meanwhile waits for it (`save_action`).
    let refreshing = RwSignal::new(false);
    let deferred: RwSignal<Option<(bool, Option<String>, Option<&'static str>)>> =
        RwSignal::new(None);
    let loading_earlier = RwSignal::new(false);

    let controller =
        StoredValue::new_local(web_sys::AbortController::new().expect("AbortController"));
    on_cleanup(move || controller.with_value(|c| c.abort()));

    let composer = NodeRef::<html::Textarea>::new();
    let pending: StoredValue<RefCell<Option<PendingOp>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(None));
    let steer_op: StoredValue<RefCell<Option<PendingOp>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(None));
    let viewport = NodeRef::<html::Div>::new();
    let content = NodeRef::<html::Div>::new();
    let following = StoredValue::new_local(RefCell::new(true));
    let positioned = StoredValue::new_local(RefCell::new(false));
    let prepend: StoredValue<RefCell<Option<(i32, i32)>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(None));
    let find_input = NodeRef::<html::Input>::new();
    let at_end = RwSignal::new(true);
    let find = RwSignal::new(String::new());
    let match_idx = RwSignal::new(0usize);
    let copy_error = RwSignal::new(String::new());

    let (model, effort_raw, effort) = use_model_choice(provider, key.clone());
    let running = Memo::new(move |_| {
        if let Some(r) = run.get() {
            active_agent_run(&r.status)
        } else {
            history.with(|h| {
                h.as_ref()
                    .is_some_and(|h| h.runs.iter().any(|item| active_agent_run(&item.status)))
            })
        }
    });

    let queued: RwSignal<Vec<String>> = RwSignal::new(
        storage_get(session_storage(), &format!("{key}.queued"))
            .map(|raw| {
                serde_json::from_str::<Vec<String>>(&raw)
                    .unwrap_or_else(|_| if raw.is_empty() { Vec::new() } else { vec![raw] })
            })
            .unwrap_or_default(),
    );
    let stash_open = RwSignal::new(false);
    let plan = RwSignal::new(
        storage_get(session_storage(), &format!("{key}.mode")).as_deref() == Some("plan"),
    );
    let images = PicturesState::new(Some(thread_id.clone()), session.csrf_token.clone());

    // Consume external composer fills (e.g. Side Chat "Use in composer").
    Effect::new(move |_| {
        if let Some(text) = incoming_draft.get() {
            if !text.is_empty() {
                draft.set(text);
                if let Some(el) = composer.get_untracked() {
                    let _ = el.focus();
                }
            }
            incoming_draft.set(None);
        }
    });

    // Auto-clear transient status notices so stale "Task sent to Codex." never lingers permanently.
    Effect::new(move |_| {
        let msg = notice.get();
        let transient = msg.starts_with("Task sent to ")
            || msg == "Sent to the running turn."
            || msg == "Queued. It is sent when the agent finishes."
            || msg == "Copied to clipboard."
            || msg == "Prompt stashed.";
        if transient {
            let expected = msg.clone();
            spawn_local(async move {
                crate::sleep(3500).await;
                if notice.try_get_untracked().as_deref() == Some(expected.as_str()) {
                    notice.try_set(String::new());
                }
            });
        }
    });

    Effect::new(move |_| {
        let p = plan.get();
        let k = format!("{}.mode", key_sv.get_value());
        if p {
            let _ = storage_set(session_storage(), &k, "plan");
        } else {
            storage_remove(session_storage(), &k);
        }
    });

    if let Some(files) = initial_files
        && !files.is_empty()
    {
        let raw_files: Vec<web_sys::File> = files.into_iter().map(|f| f.take()).collect();
        let msg = images.add(raw_files);
        if !msg.is_empty() {
            error.set(msg);
        }
    }

    let add_images = Callback::new(move |files: Vec<web_sys::File>| {
        let msg = images.add(files);
        if !msg.is_empty() {
            error.set(msg);
        }
    });

    Effect::new(move |_| {
        let q = queued.get();
        let k = format!("{}.queued", key_sv.get_value());
        if !q.is_empty() {
            if let Ok(raw) = serde_json::to_string(&q) {
                let _ = storage_set(session_storage(), &k, &raw);
            }
        } else {
            storage_remove(session_storage(), &k);
        }
    });

    // Focus composer on mount.
    Effect::new(move |_| {
        if let Some(el) = composer.get() {
            focus_prevent_scroll(&el);
        }
    });

    // Auto-size textarea rows.
    Effect::new(move |_| {
        let text = draft.get();
        if let Some(field) = composer.get() {
            let rows: usize = text
                .split('\n')
                .map(|line| ((line.chars().count() as f64 / 75.0).ceil() as usize).max(1))
                .sum();
            field.set_rows(rows.clamp(2, 7) as u32);
        }
    });

    Effect::new(move |_| {
        if find_open.get() {
            if let Some(el) = find_input.get() {
                let _ = el.focus();
            }
        } else {
            find.set(String::new());
        }
    });

    // Persist scroll position on unmount.
    on_cleanup(move || {
        if positioned.with_value(|p| *p.borrow())
            && let Some(el) = viewport.get_untracked()
        {
            let tid = thread_id_sv.get_value();
            let fol = following.with_value(|f| *f.borrow());
            positions.with_value(|map| {
                map.borrow_mut().insert(
                    tid,
                    ScrollPosition {
                        top: el.scroll_top(),
                        following: fol,
                    },
                );
            });
        }
    });

    // Keep scrolled to bottom or preserve scroll on prepend.
    Effect::new(move |_| {
        history.track();
        run.with(|r| {
            if let Some(r) = r {
                let _ = r.document.output.len();
                let _ = r.document.activity.len();
            }
        });
        let Some(el) = viewport.get() else { return };
        if history.with_untracked(Option::is_none) {
            return;
        }
        if let Some((prev_h, prev_top)) = prepend.with_value(|p| p.borrow_mut().take()) {
            el.set_scroll_top(prev_top + el.scroll_height() - prev_h);
        } else if !positioned.with_value(|p| *p.borrow()) {
            let tid = thread_id_sv.get_value();
            let saved = positions.with_value(|m| m.borrow().get(&tid).copied());
            let fol = saved.map(|s| s.following).unwrap_or(true);
            following.with_value(|f| *f.borrow_mut() = fol);
            el.set_scroll_top(if fol {
                el.scroll_height()
            } else {
                saved.map(|s| s.top).unwrap_or(0)
            });
            positioned.with_value(|p| *p.borrow_mut() = true);
        } else if following.with_value(|f| *f.borrow()) {
            el.set_scroll_top(el.scroll_height());
        }
        at_end.set(el.scroll_height() - el.scroll_top() - el.client_height() < 48);
    });

    // ResizeObserver to keep pinned to bottom.
    Effect::new(move |_| {
        let (Some(vp), Some(node)) = (viewport.get(), content.get()) else {
            return;
        };
        let vp_cl = vp.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            if following.with_value(|f| *f.borrow()) && positioned.with_value(|p| *p.borrow()) {
                vp_cl.set_scroll_top(vp_cl.scroll_height());
            }
        });
        let observer =
            web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()).expect("ResizeObserver");
        observer.observe(&node);
        observer.observe(&vp);
        let kept = SendWrapper::new((observer, cb));
        on_cleanup(move || {
            let (obs, _cb) = kept.take();
            obs.disconnect();
        });
    });

    // Load thread history.
    Effect::new(move |_| {
        let _r = reload.get();
        let _ep = history_epoch.get();
        let tid = thread_id_sv.get_value();
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let abort_keep = SendWrapper::new(abort);
        on_cleanup(move || abort_keep.abort());
        error.set(String::new());
        refreshing.set(true);
        spawn_local(async move {
            match api::request::<AgentHistory>(
                &format!("{BASE}/threads/{tid}/messages"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(value) => {
                    if signal.aborted() {
                        return;
                    }
                    let updated_thread = value.thread.clone();
                    let first_run = value.runs.first().map(|r| r.id.clone());
                    history.try_set(Some(value));
                    refreshing.try_set(false);
                    {
                        on_changed.try_run(updated_thread);
                    };
                    if let Some(rid) = first_run {
                        {
                            on_run.try_run(rid);
                        };
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        refreshing.try_set(false);
                        error.try_set(failure(&e));
                    }
                }
            }
        });
    });

    // Save draft to cache & sessionStorage.
    Effect::new(move |_| {
        let text = draft.get();
        let k = key_sv.get_value();
        draft_cache.with_value(|c| {
            c.borrow_mut().insert(k.clone(), text.clone());
        });
        let res = if !text.is_empty() {
            storage_set(session_storage(), &k, &text)
        } else {
            storage_remove(session_storage(), &k);
            Ok(())
        };
        if res.is_ok() {
            storage_error.set(String::new());
        } else {
            storage_error.set(
                "Draft recovery is unavailable in this browser. Save your message before leaving."
                    .into(),
            );
        }
    });

    // beforeunload protection when storage failed.
    listen(&window(), "beforeunload", false, move |event| {
        if !storage_error.with_untracked(String::is_empty)
            && !draft.with_untracked(String::is_empty)
        {
            event.prevent_default();
        }
    });

    let steer = move |text: String| {
        let Some(cur_run) = run.get_untracked() else {
            return;
        };
        if !active_agent_run(&cur_run.status) {
            return;
        }
        let op = steer_op.with_value(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.as_ref().is_none_or(|p| p.content != text) {
                *slot = Some(PendingOp {
                    id: random_uuid(),
                    content: text.clone(),
                });
            }
            slot.clone().unwrap()
        });
        busy.set(true);
        error.set(String::new());
        let run_id = cur_run.id;
        let signal = controller.with_value(|c| c.signal());
        let token = csrf.get_value();
        let body = serde_json::to_string(&op).unwrap_or_default();
        spawn_local(async move {
            let res = api::request::<MessageCreated>(
                &format!("{BASE}/runs/{run_id}/steer"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await;
            if signal.aborted() {
                return;
            }
            match res {
                Ok(val) => {
                    steer_op.with_value(|c| *c.borrow_mut() = None);
                    history.try_update(|h| {
                        if let Some(cur) = h {
                            cur.thread = val.thread.clone();
                            cur.items = merge_messages(&cur.items, &[val.message]);
                        }
                    });
                    {
                        on_changed.try_run(val.thread);
                    };
                    draft.try_set(String::new());
                    let k = key_sv.get_value();
                    draft_cache.with_value(|c| {
                        c.borrow_mut().insert(k.clone(), String::new());
                    });
                    storage_remove(session_storage(), &k);
                    notice.try_set("Sent to the running turn.".into());
                    following.with_value(|f| *f.borrow_mut() = true);
                }
                Err(e) => {
                    error.try_set(failure(&e));
                }
            }
            busy.try_set(false);
        });
    };

    let save_action = Callback::new(
        move |(execute, text_opt, special): (bool, Option<String>, Option<&'static str>)| {
            let cur_draft = draft.get_untracked();
            let requested = text_opt.clone();
            let from_queue = text_opt.is_some() && special.is_none();
            let chips = if !from_queue && special.is_none() {
                context_chips.get_untracked()
            } else {
                Vec::new()
            };
            let raw_text = text_opt.unwrap_or_else(|| cur_draft.clone());
            let text = if chips.is_empty() {
                raw_text
            } else {
                let prefix = chips
                    .iter()
                    .map(|c| c.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if raw_text.trim().is_empty() {
                    prefix
                } else {
                    format!("{prefix}\n\n{raw_text}")
                }
            };
            let Some(cur_hist) = history.get_untracked() else {
                return;
            };
            if text.trim().is_empty() || busy.get_untracked() {
                return;
            }
            if running.get_untracked() {
                if execute && !from_queue {
                    if follow_up_mode.get_untracked() == "queue" {
                        queued.update(|list| {
                            if list.len() < 8 {
                                list.push(text.clone());
                            }
                        });
                        draft.set(String::new());
                        context_chips.set(Vec::new());
                        let k = key_sv.get_value();
                        draft_cache.with_value(|c| {
                            c.borrow_mut().insert(k, String::new());
                        });
                        notice.set("Queued. It is sent when the agent finishes.".into());
                    } else {
                        context_chips.set(Vec::new());
                        steer(text);
                    }
                }
                return;
            }
            // A turn that just ended changed the thread: its revision arrives with the history being
            // reloaded, and sending before that would be refused as a conflict.
            if refreshing.get_untracked() {
                deferred.set(Some((execute, requested, special)));
                return;
            }
            let has_pics = images.items.with_untracked(|v| !v.is_empty());
            if special.is_none() && has_pics && (!execute || images.busy() || images.failed()) {
                error.set(if !execute {
                    "Images are sent with an agent run, not saved on their own.".into()
                } else if images.failed() {
                    "Remove images that failed to upload.".into()
                } else {
                    "Wait for images to finish uploading.".into()
                });
                return;
            }
            if text.len() > AGENT_MESSAGE_MAX_BYTES {
                error.set("Keep each message within 16 KiB.".into());
                return;
            }
            busy.set(true);
            error.set(String::new());
            notice.set(String::new());

            let op_key = operation_key.get_value();
            let op = pending.with_value(|cell| {
                let mut slot = cell.borrow_mut();
                if slot.as_ref().is_none_or(|p| p.content != text) {
                    let saved = storage_get(session_storage(), &op_key)
                        .and_then(|raw| serde_json::from_str::<PendingOp>(&raw).ok())
                        .filter(|p| p.content == text);
                    *slot = Some(saved.unwrap_or_else(|| PendingOp {
                        id: random_uuid(),
                        content: text.clone(),
                    }));
                }
                slot.clone().unwrap()
            });
            if let Ok(json_op) = serde_json::to_string(&op) {
                let _ = storage_set(session_storage(), &op_key, &json_op);
            }

            let tid = thread_id_sv.get_value();
            let revision = cur_hist.thread.revision;
            let signal = controller.with_value(|c| c.signal());
            let token = csrf.get_value();
            let k = key_sv.get_value();

            if execute {
                let cur_model = model.get_untracked();
                let cur_effort = effort.get_untracked();
                let cur_plan = plan.get_untracked();
                let mode_str = special
                    .map(str::to_owned)
                    .or_else(|| cur_plan.then(|| "plan".to_owned()));
                let attachments: Vec<String> = if from_queue || special.is_some() {
                    Vec::new()
                } else {
                    images
                        .items
                        .with_untracked(|list| list.iter().map(|i| i.id.clone()).collect())
                };
                let run_key = format!("{k}.run-operation");
                let mut req_op = RunOp {
                    id: random_uuid(),
                    message_id: op.id.clone(),
                    content: op.content.clone(),
                    model: cur_model.clone(),
                    effort: (!cur_effort.is_empty()).then(|| cur_effort.clone()),
                    mode: mode_str.clone(),
                    attachments: (!attachments.is_empty()).then(|| attachments.clone()),
                };
                if let Some(saved) = storage_get(session_storage(), &run_key)
                    .and_then(|raw| serde_json::from_str::<RunOp>(&raw).ok())
                    && saved.content == text
                    && saved.model == cur_model
                    && saved.effort.as_deref().unwrap_or("") == cur_effort
                    && saved.mode.as_deref().unwrap_or("") == mode_str.as_deref().unwrap_or("")
                    && saved.attachments.as_deref().unwrap_or(&[]) == attachments.as_slice()
                {
                    req_op = saved;
                }
                if let Ok(raw) = serde_json::to_string(&req_op) {
                    let _ = storage_set(session_storage(), &run_key, &raw);
                }
                let body = serde_json::to_string(&req_op).unwrap_or_default();
                let if_match = format!("\"{revision}\"");
                spawn_local(async move {
                    let res = api::request::<AgentRun>(
                        &format!("{BASE}/threads/{tid}/runs"),
                        Req {
                            method: "POST",
                            body: Some(body),
                            csrf: Some(&token),
                            headers: &[("if-match", &if_match)],
                            signal: Some(signal.clone()),
                            ..Default::default()
                        },
                    )
                    .await;
                    if signal.aborted() {
                        return;
                    }
                    match res {
                        Ok(value) => {
                            {
                                on_run.try_run(value.id);
                            };
                            reload.try_update(|n| *n += 1);
                            pending.with_value(|c| *c.borrow_mut() = None);
                            if !from_queue && special.is_none() {
                                images.clear();
                                context_chips.try_set(Vec::new());
                            }
                            if from_queue {
                                queued.try_update(|list| {
                                    if !list.is_empty() {
                                        list.remove(0);
                                    }
                                });
                            } else if special.is_none() {
                                draft.try_set(String::new());
                                draft_cache.with_value(|c| {
                                    c.borrow_mut().insert(k.clone(), String::new());
                                });
                            }
                            storage_remove(session_storage(), &op_key);
                            storage_remove(session_storage(), &run_key);
                            if !from_queue && special.is_none() {
                                storage_remove(session_storage(), &k);
                            }
                            notice.try_set(format!("Task sent to {}.", provider_name(&cur_model)));
                        }
                        Err(e) => {
                            error.try_set(failure(&e));
                        }
                    }
                    busy.try_set(false);
                });
            } else {
                let body = serde_json::to_string(&op).unwrap_or_default();
                let if_match = format!("\"{revision}\"");
                spawn_local(async move {
                    let res = api::request::<MessageCreated>(
                        &format!("{BASE}/threads/{tid}/messages"),
                        Req {
                            method: "POST",
                            body: Some(body),
                            csrf: Some(&token),
                            headers: &[("if-match", &if_match)],
                            signal: Some(signal.clone()),
                            ..Default::default()
                        },
                    )
                    .await;
                    if signal.aborted() {
                        return;
                    }
                    match res {
                        Ok(value) => {
                            history.try_update(|h| {
                                if let Some(cur) = h {
                                    cur.thread = value.thread.clone();
                                    cur.items = merge_messages(&cur.items, &[value.message]);
                                }
                            });
                            {
                                on_changed.try_run(value.thread);
                            };
                            draft.try_set(String::new());
                            draft_cache.with_value(|c| {
                                c.borrow_mut().insert(k.clone(), String::new());
                            });
                            pending.with_value(|c| *c.borrow_mut() = None);
                            storage_remove(session_storage(), &op_key);
                            storage_remove(session_storage(), &k);
                            notice
                                .try_set("Message saved. It has not been sent to a model.".into());
                            if let Some(el) = composer.get_untracked() {
                                let _ = el.focus();
                            }
                            following.with_value(|f| *f.borrow_mut() = true);
                            request_animation_frame(move || {
                                if let Some(vp) = viewport.get_untracked() {
                                    vp.set_scroll_top(vp.scroll_height());
                                }
                            });
                        }
                        Err(e) => {
                            error.try_set(failure(&e));
                        }
                    }
                    busy.try_set(false);
                });
            }
        },
    );
    // A send held while the history reloaded goes out once it has loaded.
    Effect::new(move |_| {
        if !refreshing.get()
            && let Some(args) = deferred.try_update(Option::take).flatten()
        {
            save_action.run(args);
        }
    });

    // Listen for `dar:agent-special` custom events (review / compact from thread menu).
    listen(&window(), "dar:agent-special", false, move |event| {
        if let Ok(ce) = event.dyn_into::<web_sys::CustomEvent>() {
            let detail = ce.detail();
            let t = js_sys::Reflect::get(&detail, &JsValue::from_str("thread"))
                .ok()
                .and_then(|v| v.as_string());
            let m = js_sys::Reflect::get(&detail, &JsValue::from_str("mode"))
                .ok()
                .and_then(|v| v.as_string());
            if t.as_deref() == Some(thread_id_sv.get_value().as_str()) {
                match m.as_deref() {
                    Some("review") => save_action.run((
                        true,
                        Some("Review the uncommitted changes in this working copy.".into()),
                        Some("review"),
                    )),
                    Some("compact") => save_action.run((
                        true,
                        Some("Compact the conversation context.".into()),
                        Some("compact"),
                    )),
                    _ => {}
                }
            }
        }
    });

    // Auto-start draft composed before thread existed.
    let started_flag = StoredValue::new_local(RefCell::new(false));
    Effect::new(move |_| {
        let Some(auto) = auto_start else { return };
        if started_flag.with_value(|f| *f.borrow()) {
            return;
        }
        let has_hist = history.with(Option::is_some);
        let checking = checking_provider.get();
        let pics_busy = images.busy();
        let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
        if !has_hist
            || draft.with_untracked(|d| d.trim().is_empty())
            || (auto && checking)
            || pics_busy
        {
            return;
        }
        started_flag.with_value(|f| *f.borrow_mut() = true);
        {
            on_started.try_run(());
        };
        save_action.run((auto && connected, None, None));
    });

    // Auto-send queued follow-up when previous run completes.
    Effect::new(move |_| {
        let next_item = queued.with(|list| list.first().cloned());
        let is_running = running.get();
        let is_busy = busy.get();
        let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
        let latest_completed = history.with(|h| {
            h.as_ref()
                .and_then(|h| h.runs.first())
                .map(|r| (r.id.clone(), r.status == "completed"))
        });
        if let Some(q) = next_item
            && !is_running
            && !is_busy
            && connected
            && latest_completed.is_some_and(|(_, done)| done)
        {
            save_action.run((true, Some(q), None));
        }
    });

    let fork = move |sequence: u64| {
        let tid = thread_id_sv.get_value();
        let signal = controller.with_value(|c| c.signal());
        let token = csrf.get_value();
        let body = json!({ "id": random_uuid(), "sequence": sequence }).to_string();
        spawn_local(async move {
            match api::request::<AgentThread>(
                &format!("{BASE}/threads/{tid}/fork"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(row) => {
                    if !signal.aborted() {
                        {
                            on_forked.try_run(row);
                        };
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        error.try_set(failure(&e));
                    }
                }
            }
        });
    };

    let queue_follow_up = move || {
        let text = draft.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        if queued.with_untracked(Vec::len) >= 8 {
            error.set("At most 8 follow-ups can be queued at once.".into());
            return;
        }
        queued.update(|list| list.push(text));
        draft.set(String::new());
        let k = key_sv.get_value();
        draft_cache.with_value(|c| {
            c.borrow_mut().insert(k, String::new());
        });
        notice.set("Queued. It is sent when the agent finishes.".into());
    };

    let earlier = move || {
        let next_before = history.with_untracked(|h| h.as_ref().and_then(|h| h.next_before));
        let Some(before) = next_before else { return };
        if loading_earlier.get_untracked() {
            return;
        }
        loading_earlier.set(true);
        error.set(String::new());
        let tid = thread_id_sv.get_value();
        let signal = controller.with_value(|c| c.signal());
        spawn_local(async move {
            let res = api::request::<AgentHistory>(
                &format!("{BASE}/threads/{tid}/messages?before={before}"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await;
            if signal.aborted() {
                return;
            }
            match res {
                Ok(value) => {
                    if let Some(el) = viewport.get_untracked() {
                        prepend.with_value(|p| {
                            *p.borrow_mut() = Some((el.scroll_height(), el.scroll_top()))
                        });
                    }
                    history.try_update(|h| {
                        if let Some(cur) = h {
                            cur.items = merge_messages(&cur.items, &value.items);
                            cur.next_before = value.next_before;
                        }
                    });
                }
                Err(e) => {
                    error.try_set(failure(&e));
                }
            }
            loading_earlier.try_set(false);
        });
    };

    let matches = Memo::new(move |_| {
        let needle = find.get().trim().to_lowercase();
        if needle.is_empty() {
            return Vec::<String>::new();
        }
        history.with(|h| {
            h.as_ref()
                .map(|h| {
                    h.items
                        .iter()
                        .filter(|m| m.content.to_lowercase().contains(&needle))
                        .map(|m| m.id.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    });

    let match_id = Memo::new(move |_| {
        matches.with(|list| {
            if list.is_empty() {
                None
            } else {
                list.get(match_idx.get() % list.len()).cloned()
            }
        })
    });

    Effect::new(move |_| {
        let Some(mid) = match_id.get() else { return };
        following.with_value(|f| *f.borrow_mut() = false);
        if let Some(vp) = viewport.get()
            && let Ok(nodes) = vp.query_selector_all("[data-message-id]")
        {
            for i in 0..nodes.length() {
                if let Some(el) = nodes
                    .item(i)
                    .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
                    && el.get_attribute("data-message-id").as_deref() == Some(mid.as_str())
                {
                    scroll_into_view_block(&el, web_sys::ScrollLogicalPosition::Center);
                    break;
                }
            }
        }
    });

    let copy_text = move |text: String| {
        spawn_local(async move {
            match write_clipboard(&text).await {
                Ok(()) => {
                    notice.try_set("Copied to clipboard.".into());
                    copy_error.try_set(String::new());
                }
                Err(()) => {
                    copy_error
                        .try_set("Could not copy. Select the message text to copy it.".into());
                }
            }
        });
    };

    let slash_commands = Signal::derive(move || {
        let cur_model = model.get();
        let efforts = provider.with(|p| {
            p.as_ref()
                .and_then(|p| p.models.iter().find(|m| m.id == cur_model))
                .and_then(|m| m.efforts.clone())
                .unwrap_or_default()
        });
        let mut list = commands.run(SlashControls {
            set_effort: Callback::new(move |val: String| effort_raw.set(val)),
            efforts,
        });
        list.push(SlashCommand {
            name: "plan".into(),
            hint: "Toggle plan mode for the next turn".into(),
            argument: None,
            run: Callback::new(move |_| plan.update(|v| *v = !*v)),
        });
        list.push(SlashCommand {
            name: "review".into(),
            hint: "Ask the agent to review uncommitted changes".into(),
            argument: None,
            run: Callback::new(move |_| {
                save_action.run((
                    true,
                    Some("Review the uncommitted changes in this working copy.".into()),
                    Some("review"),
                ))
            }),
        });
        if provider_of(&cur_model) != "claude" {
            list.push(SlashCommand {
                name: "compact".into(),
                hint: "Compact the conversation context".into(),
                argument: None,
                run: Callback::new(move |_| {
                    save_action.run((
                        true,
                        Some("Compact the conversation context.".into()),
                        Some("compact"),
                    ))
                }),
            });
        }
        list
    });

    let prompts_sig = Signal::derive(move || {
        history.with(|h| {
            h.as_ref()
                .map(|h| {
                    h.items
                        .iter()
                        .filter(|m| m.role == "user")
                        .map(|m| m.content.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    });

    let placeholder_sig = Signal::derive(move || {
        if running.get() {
            "Steer the running turn, or queue a follow-up\u{2026}".into()
        } else if provider.with(|p| p.as_ref().is_some_and(|p| p.connected)) {
            format!(
                "Ask {} to plan, change or explain\u{2026} @ files, / commands",
                provider_name(&model.get())
            )
        } else {
            "Write a message to save\u{2026}".into()
        }
    });

    let minimap_version = Signal::derive(move || {
        let len = history.with(|h| {
            h.as_ref()
                .map(|h| h.items.len().to_string())
                .unwrap_or_default()
        });
        let (st, steps) = run.with(|r| {
            r.as_ref()
                .map(|r| {
                    (
                        r.status.clone(),
                        r.document
                            .steps
                            .as_ref()
                            .map(|s| s.len().to_string())
                            .unwrap_or_default(),
                    )
                })
                .unwrap_or_default()
        });
        format!("{len}-{st}-{steps}")
    });

    const TRUNC_SUFFIX: &str = "\n\n[Full response is in the run output.]";

    view! {
        {move || find_open.get().then(|| view! {
            <div class="agent-find">
                <Icon name="search"/>
                <input node_ref=find_input aria-label="Find in thread" type="search" placeholder="Find in loaded messages\u{2026}"
                    prop:value=move || find.get()
                    on:input=move |e| { find.set(event_target_value(&e)); match_idx.set(0); }
                    on:keydown=move |e| {
                        if e.key() == "Escape" {
                            e.prevent_default();
                            e.stop_propagation();
                            { on_close_find.try_run(()); };
                            if let Some(el) = composer.get_untracked() { let _ = el.focus(); }
                        } else if e.key() == "Enter" {
                            e.prevent_default();
                            let len = matches.with_untracked(Vec::len);
                            if len > 0 {
                                let delta = if e.shift_key() { len - 1 } else { 1 };
                                match_idx.update(|v| *v = (*v + delta) % len);
                            }
                        }
                    }/>
                <span role="status">
                    {move || {
                        let len = matches.with(Vec::len);
                        if len > 0 {
                            format!("{} / {len}", (match_idx.get() % len) + 1)
                        } else if !find.with(String::is_empty) {
                            "No matches".into()
                        } else {
                            "Loaded messages".into()
                        }
                    }}
                </span>
                <button class="icon-button" aria-label="Next match" disabled=move || matches.with(Vec::is_empty)
                    on:click=move |_| {
                        let len = matches.with_untracked(Vec::len);
                        if len > 0 { match_idx.update(|v| *v = (*v + 1) % len); }
                    }>
                    <Icon name="chevron"/>
                </button>
                <button class="icon-button" aria-label="Close find" on:click=move |_| {
                    { on_close_find.try_run(()); };
                    if let Some(el) = composer.get_untracked() { let _ = el.focus(); }
                }>
                    <Icon name="close"/>
                </button>
            </div>
        })}
        <div class="agent-timeline-wrap">
            <div node_ref=viewport class="agent-conversation" aria-label="Conversation messages"
                aria-busy=move || (history.with(Option::is_none) && error.with(String::is_empty)).to_string()
                on:scroll=move |_| {
                    if let Some(el) = viewport.get_untracked() {
                        let end = el.scroll_height() - el.scroll_top() - el.client_height() < 48;
                        following.with_value(|f| *f.borrow_mut() = end);
                        at_end.set(end);
                    }
                }>
                <div class="agent-timeline-content" node_ref=content>
                    {move || (history.with(Option::is_none) && error.with(String::is_empty)).then(|| view! { <p role="status">"Loading conversation\u{2026}"</p> })}
                    {move || history.with(|h| h.as_ref().and_then(|h| h.next_before)).is_some().then(|| view! {
                        <button class="text-button" disabled=move || loading_earlier.get() on:click=move |_| earlier()>
                            {move || if loading_earlier.get() { "Loading\u{2026}" } else { "Load earlier messages" }}
                        </button>
                    })}
                    {move || {
                        let empty = history.with(|h| h.as_ref().is_some_and(|h| h.items.is_empty()));
                        (empty && auto_start.is_none()).then(|| {
                            let pname = project.with(|p| p.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "this project".into()));
                            let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
                            let prov = provider_name(&model.get());
                            view! {
                                <div class="agent-empty agent-hero">
                                    <h2>"What should we build in "<strong>{pname}</strong>"?"</h2>
                                    <p>{if connected { format!("{prov} works in an isolated copy of this project.") } else { "No agent is connected. Messages are saved here until Codex or Claude Code is connected.".into() }}</p>
                                </div>
                            }
                        })
                    }}
                    {
                        #[derive(Clone, PartialEq)]
                        struct MessageRow {
                            message: AgentMessage,
                            show_saved_label: bool,
                            run_matches_msg: bool,
                            attachments: Vec<AgentAttachment>,
                            is_plan_msg: bool,
                            earlier_runs: Vec<AgentRunSummary>,
                        }
                        let run_meta = Memo::new(move |_| {
                            run.with(|r| {
                                r.as_ref().map(|r| {
                                    (
                                        r.message_id.clone(),
                                        r.document.steered.clone().unwrap_or_default(),
                                        r.document.attachments.clone().unwrap_or_default(),
                                        r.document.mode.clone(),
                                    )
                                })
                            })
                        });
                        let message_rows = Memo::new(move |_| {
                            let Some(hist) = history.get() else { return Vec::new() };
                            let meta = run_meta.get();
                            let all_items = &hist.items;
                            let hist_runs = &hist.runs;
                            all_items
                                .iter()
                                .map(|message| {
                                    let is_user = message.role == "user";
                                    let has_run_for_msg = hist_runs.iter().any(|item| {
                                        item.message_id == message.id
                                            || (message.created_at >= item.created_at
                                                && message.created_at <= item.modified_at
                                                && item.message_id != message.id
                                                && all_items.iter().any(|other| other.id == item.message_id && other.sequence < message.sequence))
                                    });
                                    let steered_here = meta.as_ref().is_some_and(|(_, steered, _, _)| steered.contains(&message.id));
                                    let show_saved_label = is_user && !has_run_for_msg && !steered_here;
                                    let run_matches_msg = meta.as_ref().is_some_and(|(mid, _, _, _)| mid == &message.id);
                                    let attachments = if is_user && run_matches_msg {
                                        meta.as_ref().map(|(_, _, att, _)| att.clone()).unwrap_or_default()
                                    } else {
                                        Vec::new()
                                    };
                                    let is_plan_msg = is_user
                                        && run_matches_msg
                                        && meta.as_ref().and_then(|(_, _, _, mode)| mode.as_deref()) == Some("plan");
                                    let earlier_runs: Vec<AgentRunSummary> = if run_matches_msg {
                                        Vec::new()
                                    } else {
                                        hist_runs.iter().filter(|r| r.message_id == message.id).cloned().collect()
                                    };
                                    MessageRow {
                                        message: message.clone(),
                                        show_saved_label,
                                        run_matches_msg,
                                        attachments,
                                        is_plan_msg,
                                        earlier_runs,
                                    }
                                })
                                .collect::<Vec<_>>()
                        });
                        view! {
                            <For
                                each=move || message_rows.get()
                                key=|row| {
                                    use std::hash::{Hash, Hasher};
                                    let mut h = std::collections::hash_map::DefaultHasher::new();
                                    row.message.role.hash(&mut h);
                                    row.message.content.hash(&mut h);
                                    row.message.sequence.hash(&mut h);
                                    row.message.created_at.to_bits().hash(&mut h);
                                    row.show_saved_label.hash(&mut h);
                                    row.run_matches_msg.hash(&mut h);
                                    row.is_plan_msg.hash(&mut h);
                                    row.attachments.len().hash(&mut h);
                                    for att in &row.attachments {
                                        att.id.hash(&mut h);
                                        att.name.hash(&mut h);
                                    }
                                    row.earlier_runs.len().hash(&mut h);
                                    for r in &row.earlier_runs {
                                        r.id.hash(&mut h);
                                        r.status.hash(&mut h);
                                        r.modified_at.to_bits().hash(&mut h);
                                    }
                                    (row.message.id.clone(), h.finish())
                                }
                                let:row
                            >
                                {
                                    let message = row.message;
                                    let is_user = message.role == "user";
                                    let msg_id_match = message.id.clone();
                                    let msg_content_copy = message.content.clone();
                                    let msg_seq = message.sequence;
                                    let time_iso = iso_string(message.created_at);
                                    let time_disp = local_time(message.created_at, &[("hour", "numeric"), ("minute", "2-digit")]);
                                    let tid_att = thread_id_sv.get_value();
                                    view! {
                                        <article data-message-id=message.id.clone() data-match=move || (match_id.get().as_deref() == Some(msg_id_match.as_str())).then_some("true") class=format!("agent-message agent-message-{}", message.role)>
                                            <div class="agent-message-body">
                                                {if message.role == "assistant" {
                                                    view! { <Markdown source=message.content.clone()/> }.into_any()
                                                } else {
                                                    view! { <p>{message.content.clone()}</p> }.into_any()
                                                }}
                                            </div>
                                            {row.show_saved_label.then(|| view! { <small class="agent-saved-label">"Saved \u{00b7} not sent to a model"</small> })}
                                            {(!row.attachments.is_empty()).then(|| view! {
                                                <div class="agent-message-images">
                                                    {row.attachments.into_iter().map(|att| {
                                                        let href = format!("{BASE}/threads/{tid_att}/attachments/{}", att.id);
                                                        view! { <a href=href.clone() target="_blank" rel="noreferrer"><img src=href.clone() alt=att.name loading="lazy"/></a> }
                                                    }).collect_view()}
                                                </div>
                                            })}
                                            {row.is_plan_msg.then(|| view! { <small class="agent-saved-label">"Plan mode"</small> })}
                                            <div class="agent-message-meta">
                                                {is_user.then(|| view! { <time datetime=time_iso.clone()>{time_disp.clone()}</time> })}
                                                <button class="icon-button" aria-label=if is_user { "Copy message" } else { "Copy response" } title="Copy" on:click=move |_| copy_text(msg_content_copy.clone())><Icon name="copy"/></button>
                                                <button class="icon-button" aria-label="Fork from here" title="Fork a new thread from this message" disabled=move || running.get() on:click=move |_| fork(msg_seq)><Icon name="share"/></button>
                                                {(!is_user).then(|| view! { <time datetime=time_iso>{time_disp}</time> })}
                                            </div>
                                        </article>
                                        {if row.run_matches_msg {
                                            let r_sig = Signal::from(Memo::new(move |last| run.get().or_else(|| last.cloned()).expect("run present")));
                                            view! { <WorkLog run=r_sig on_review=on_review on_open_file=on_open_file/> }.into_any()
                                        } else {
                                            row.earlier_runs.into_iter().map(|s| view! { <EarlierRun summary=s/> }).collect_view().into_any()
                                        }}
                                    }
                                }
                            </For>
                        }
                    }
                    {
                        let live_state = Memo::new(move |_| {
                            let output = run.with(|r| r.as_ref().map(|r| r.document.output.clone()).unwrap_or_default());
                            if output.is_empty() {
                                return None;
                            }
                            let is_running = running.get();
                            let truncated_response = history.with(|h| {
                                h.as_ref().is_some_and(|h| {
                                    h.items.iter().any(|m| {
                                        m.role == "assistant"
                                            && m.content.ends_with(TRUNC_SUFFIX)
                                            && output.starts_with(&m.content[..m.content.len() - TRUNC_SUFFIX.len()])
                                    })
                                })
                            });
                            let response_saved = truncated_response
                                || history.with(|h| {
                                    h.as_ref().is_some_and(|h| h.items.iter().any(|m| m.role == "assistant" && m.content == output))
                                });
                            let show_live = is_running || !response_saved;
                            Some((output, show_live, truncated_response))
                        });
                        move || {
                            let (output, show_live, truncated_response) = live_state.get()?;
                            Some(view! {
                                {show_live.then(|| view! {
                                    <article class="agent-message agent-message-assistant agent-live-output"><Markdown source=output.clone()/></article>
                                })}
                                {truncated_response.then(|| view! {
                                    <details class="agent-full-response"><summary>"Full response"</summary><Markdown source=output.clone()/></details>
                                })}
                            })
                        }
                    }
                    {move || {
                        let cur_run = run.get()?;
                        let is_plan_done = cur_run.status == "completed"
                            && cur_run.document.mode.as_deref() == Some("plan")
                            && !running.get();
                        let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
                        let plan_copy = cur_run.document.output.clone();
                        is_plan_done.then(|| view! {
                            <div class="agent-proposed-plan">
                                <div class="agent-proposed-plan-copy">
                                    <Icon name="list"/>
                                    <div>
                                        <strong>"Proposed plan ready"</strong>
                                        <span>"Switch out of Plan mode to implement these steps in the working copy."</span>
                                    </div>
                                </div>
                                <div class="agent-proposed-plan-actions">
                                    <button type="button" class="secondary-button" on:click=move |_| copy_text(plan_copy.clone())>"Copy plan"</button>
                                    <button type="button" class="primary-button" disabled=!connected || busy.get() on:click=move |_| {
                                        plan.set(false);
                                        save_action.run((true, Some("Implement the proposed plan.".into()), None));
                                    }>"Implement plan"</button>
                                </div>
                            </div>
                        })
                    }}
                </div>
            </div>
            <Minimap viewport=viewport content=content version=minimap_version/>
            {move || (!at_end.get()).then(|| view! {
                <button class="agent-jump secondary-button" on:click=move |_| {
                    following.with_value(|f| *f.borrow_mut() = true);
                    if let Some(el) = viewport.get_untracked() { el.set_scroll_top(el.scroll_height()); }
                    at_end.set(true);
                }>
                    "Jump to latest "<Icon name="download"/>
                </button>
            })}
        </div>
        {move || {
            let show_q = run.with(|r| r.as_ref().is_some_and(|r| r.document.pending.is_some() && active_agent_run(&r.status)));
            show_q.then(|| {
                let r_sig = Signal::from(Memo::new(move |last| run.get().or_else(|| last.cloned()).expect("run present")));
                view! { <div class="agent-question-area"><QuestionCard run=r_sig session=session_sv.get_value()/></div> }
            })
        }}
        <form class="agent-composer-area" on:submit=move |e| {
            e.prevent_default();
            let connected = provider.with_untracked(|p| p.as_ref().is_some_and(|p| p.connected));
            save_action.run((connected, None, None));
        }>
            {move || {
                let err = error.get();
                (!err.is_empty()).then(|| view! {
                    <div role="alert" class="error">
                        <p>{err}</p>
                        <button type="button" class="text-button" disabled=move || busy.get() on:click=move |_| reload.update(|n| *n += 1)>"Refresh conversation"</button>
                    </div>
                })
            }}
            {move || { let e = storage_error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
            {move || { let e = copy_error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
            {move || {
                let list = queued.get();
                (!list.is_empty()).then(|| {
                    let is_running = running.get();
                    let latest_completed = history.with(|h| h.as_ref().and_then(|h| h.runs.first()).is_some_and(|r| r.status == "completed"));
                    let total = list.len();
                    view! {
                        <div class="agent-queued-stack" role="status" aria-label="Queued follow-ups">
                            {list.into_iter().enumerate().map(|(idx, q)| {
                                let q_send = q.clone();
                                let q_steer = q.clone();
                                let q_edit = q.clone();
                                view! {
                                    <div class="agent-queued">
                                        <Icon name="clock"/>
                                        <span>
                                            <strong>{if total == 1 { "Queued follow-up".to_owned() } else { format!("Queued #{}/{}", idx + 1, total) }}</strong>
                                            <span>{q}</span>
                                        </span>
                                        {is_running.then(|| view! {
                                            <button type="button" class="text-button" disabled=move || busy.get()
                                                on:click=move |_| {
                                                    queued.update(|l| { if idx < l.len() { l.remove(idx); } });
                                                    steer(q_steer.clone());
                                                }>"Steer now"</button>
                                        })}
                                        {(!is_running && !latest_completed && idx == 0).then(|| view! {
                                            <button type="button" class="text-button" disabled=move || busy.get() || !provider.with(|p| p.as_ref().is_some_and(|p| p.connected))
                                                on:click=move |_| save_action.run((true, Some(q_send.clone()), None))>"Send now"</button>
                                        })}
                                        <button type="button" class="text-button" on:click=move |_| {
                                            if draft.with_untracked(|d| d.trim().is_empty()) {
                                                draft.set(q_edit.clone());
                                            }
                                            queued.update(|l| { if idx < l.len() { l.remove(idx); } });
                                            if let Some(el) = composer.get_untracked() { let _ = el.focus(); }
                                        }>"Edit"</button>
                                        <button type="button" class="icon-button" aria-label="Remove queued follow-up" on:click=move |_| {
                                            queued.update(|l| { if idx < l.len() { l.remove(idx); } });
                                        }><Icon name="close"/></button>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    }
                })
            }}
            {move || {
                let chips = context_chips.get();
                (!chips.is_empty()).then(|| view! {
                    <div class="agent-context-chips" aria-label="Attached context">
                        {chips.into_iter().map(|chip| {
                            let cid = chip.id.clone();
                            let icon = match chip.kind {
                                "diff" => "diff",
                                "terminal" => "terminal",
                                "pr" => "git",
                                "quote" => "chat",
                                _ => "file",
                            };
                            view! {
                                <span class="agent-context-chip" title=chip.content>
                                    <Icon name=icon/>
                                    <span>{chip.label}</span>
                                    <button type="button" aria-label="Remove context" on:click=move |_| context_chips.update(|l| l.retain(|c| c.id != cid))><Icon name="close"/></button>
                                </span>
                            }
                        }).collect_view()}
                    </div>
                })
            }}
            {move || stash_open.get().then(|| {
                let entries = stash.get();
                let stash_full = entries.len() >= 16;
                view! {
                    <div class="agent-stash-popover" role="region" aria-label="Prompt stash">
                        <div class="agent-stash-head">
                            <strong>"Prompt stash"</strong>
                            <button type="button" class="text-button" disabled=move || draft.with(|d| d.trim().is_empty()) || stash_full
                                on:click=move |_| {
                                    let text = draft.get_untracked().trim().to_owned();
                                    if text.is_empty() {
                                        return;
                                    }
                                    let clipped: String = text.chars().take(950).collect();
                                    let mut next = stash.get_untracked();
                                    next.insert(0, PromptStashEntry {
                                        id: random_uuid(),
                                        text: clipped,
                                        created_at: js_sys::Date::now(),
                                    });
                                    next.truncate(16);
                                    on_update_stash.try_run(next);
                                    draft.set(String::new());
                                    notice.set("Prompt stashed.".into());
                                }>"Stash current draft"</button>
                            <button type="button" class="icon-button" aria-label="Close stash" on:click=move |_| stash_open.set(false)><Icon name="close"/></button>
                        </div>
                        {if entries.is_empty() {
                            view! { <p class="muted small">"Save drafts here to reuse or return to them later."</p> }.into_any()
                        } else {
                            view! {
                                <ul class="agent-stash-list">
                                    {entries.into_iter().map(|entry| {
                                        let txt_use = entry.text.clone();
                                        let id_del = entry.id.clone();
                                        view! {
                                            <li>
                                                <button type="button" class="agent-stash-use" on:click=move |_| {
                                                    draft.set(txt_use.clone());
                                                    stash_open.set(false);
                                                    if let Some(el) = composer.get_untracked() { let _ = el.focus(); }
                                                }>{entry.text}</button>
                                                <button type="button" class="icon-button" aria-label="Delete stashed prompt" on:click=move |_| {
                                                    let mut next = stash.get_untracked();
                                                    next.retain(|e| e.id != id_del);
                                                    on_update_stash.try_run(next);
                                                }><Icon name="trash"/></button>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            }.into_any()
                        }}
                    </div>
                }
            })}
            <div class=move || if plan.get() { "agent-composer is-plan" } else { "agent-composer" }
                on:dragover=move |e| {
                    if data_transfer_has_image(&e) {
                        e.prevent_default();
                        if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                            let _ = el.set_attribute("data-drop", "true");
                        }
                    }
                }
                on:dragleave=move |e| {
                    if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                        let _ = el.remove_attribute("data-drop");
                    }
                }
                on:drop=move |e| {
                    if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                        let _ = el.remove_attribute("data-drop");
                    }
                    let dropped = e.data_transfer().map(|dt| files_from_list(dt.files(), true)).unwrap_or_default();
                    if !dropped.is_empty() {
                        e.prevent_default();
                        add_images.run(dropped);
                    }
                }>
                <PictureTray pictures=images/>
                <PromptInput
                    input_ref=composer
                    value=draft
                    on_change=Callback::new(move |v| draft.set(v))
                    on_submit=Callback::new(move |_| {
                        let connected = provider.with_untracked(|p| p.as_ref().is_some_and(|p| p.connected));
                        save_action.run((connected, None, None));
                    })
                    disabled=busy
                    send_key=send_key
                    thread_id=Signal::derive(move || thread_id_sv.get_value())
                    prompts=prompts_sig
                    commands=slash_commands
                    placeholder=placeholder_sig
                    on_files=Callback::new(move |files| {
                        if provider.with_untracked(|p| p.as_ref().is_some_and(|p| p.connected)) {
                            add_images.run(files);
                        }
                    })
                />
                <div class="agent-composer-controls">
                    {move || provider.with(|p| p.as_ref().is_some_and(|p| p.connected)).then(|| view! {
                        <AttachButton on_files=add_images disabled=Signal::derive(move || busy.get() || running.get() || images.items.with(|v| v.len() >= AGENT_ATTACHMENT_MAX_COUNT))/>
                    })}
                    {move || {
                        let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
                        let configured = provider.with(|p| p.as_ref().is_some_and(|p| p.configured));
                        if connected {
                            let models_sig = Signal::derive(move || provider.with(|p| p.as_ref().map(|p| p.models.clone()).unwrap_or_default()));
                            view! {
                                <ModelPicker
                                    models=models_sig
                                    value=model
                                    on_change=Callback::new(move |id| model.set(id))
                                    effort=effort
                                    on_effort=Callback::new(move |v| effort_raw.set(v))
                                    disabled=busy
                                    starred=starred_models
                                    on_toggle_star=on_toggle_star
                                />
                            }.into_any()
                        } else {
                            view! {
                                <span class="agent-provider-state"><span class="agent-status-dot"/>{if configured { "Agent unavailable" } else { "No provider connected" }}</span>
                            }.into_any()
                        }
                    }}
                    {move || provider.with(|p| p.as_ref().is_some_and(|p| p.connected)).then(|| view! {
                        <button type="button" class="agent-plan-toggle" aria-pressed=move || plan.get().to_string() title="Plan mode: plans and asks before changing files" on:click=move |_| plan.update(|v| *v = !*v)>
                            <Icon name="list"/>"Plan"
                        </button>
                        <button type="button" class="icon-button agent-stash-btn" aria-label="Prompt stash" title="Prompt stash" aria-expanded=move || stash_open.get().to_string() on:click=move |_| stash_open.update(|v| *v = !*v)>
                            <Icon name="stash"/>
                        </button>
                        <ActionMenu label="Message actions" actions=Callback::new(move |_| {
                            let dis = busy.get_untracked() || running.get_untracked() || history.with_untracked(Option::is_none) || draft.with_untracked(|d| d.trim().is_empty());
                            vec![MenuAction::new("Save only", "file").disabled(dis).run(move || save_action.run((false, None, None)))]
                        })><Icon name="more"/></ActionMenu>
                    })}
                    <ContextMeter
                        used=Signal::derive(move || run.with(|r| r.as_ref().and_then(|r| r.document.context_used)))
                        window_size=Signal::derive(move || run.with(|r| r.as_ref().and_then(|r| r.document.context_window)))
                    />
                    {move || (running.get() && provider.with(|p| p.as_ref().is_some_and(|p| p.connected))).then(|| view! {
                        <button type="button" class="agent-queue-button" disabled=move || busy.get() || draft.with(|d| d.trim().is_empty()) || queued.with(|l| l.len() >= 8) on:click=move |_| queue_follow_up()>"Queue"</button>
                        <button class="agent-send" aria-label="Steer the running turn" title=move || format!("Send into the running turn ({})", if send_key.get() == "enter" { "Enter" } else { "Ctrl Enter" }) disabled=move || busy.get() || draft.with(|d| d.trim().is_empty())><Icon name="send"/></button>
                    })}
                    {move || if running.get() {
                        let cancelling = run.with(|r| r.as_ref().is_some_and(|r| r.status == "cancelling"));
                        view! {
                            <button type="button" class="agent-send agent-stop" aria-label=if cancelling { "Stopping\u{2026}" } else { "Stop" } title="Stop generation" disabled=cancelling on:click=move |_| { on_stop.try_run(()); }><span aria-hidden="true"/></button>
                        }.into_any()
                    } else {
                        let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
                        let prov = provider_name(&model.get());
                        let key_hint = if send_key.get() == "enter" { "Enter" } else { "Ctrl Enter" };
                        view! {
                            <button class="agent-send"
                                aria-label=move || if busy.get() { "Saving\u{2026}".into() } else if connected { format!("Run with {prov}") } else { "Save message".into() }
                                title=if connected { format!("Run with {prov} ({key_hint})") } else { "Save message".into() }
                                disabled=move || busy.get() || history.with(Option::is_none) || (draft.with(|d| d.trim().is_empty()) && context_chips.with(Vec::is_empty)) || (connected && model.with(String::is_empty))>
                                <Icon name="send"/>
                            </button>
                        }.into_any()
                    }}
                </div>
            </div>
            <div class="agent-composer-hint" role="status">{move || notice.get()}</div>
        </form>
    }
}

// ---------------------------------------------------------------------------
// DraftComposer
// ---------------------------------------------------------------------------

#[component]
fn DraftComposer(
    session: BrowserSession,
    project: AgentProject,
    #[prop(into)] provider: Signal<Option<AgentProvider>>,
    #[prop(into)] checking_provider: Signal<bool>,
    draft_cache: StoredValue<RefCell<HashMap<String, String>>, LocalStorage>,
    on_choose_project: Callback<()>,
    on_start_scratch: Callback<()>,
    on_created: Callback<(AgentThread, bool, Vec<SendWrapper<web_sys::File>>)>,
    #[prop(into)] send_key: Signal<&'static str>,
    #[prop(into)] starred_models: Signal<Vec<String>>,
    on_toggle_star: Callback<String>,
    prefill_prompt: RwSignal<Option<String>>,
    commands: Callback<SlashControls, Vec<SlashCommand>>,
) -> impl IntoView {
    let key = format!("dar.agent.draft.{}.new", session.user.id);
    let key_sv = StoredValue::new(key.clone());
    let user_id_sv = StoredValue::new(session.user.id.clone());
    let csrf = StoredValue::new(session.csrf_token.clone());
    let project_id = StoredValue::new(project.id.clone());
    let project_name = StoredValue::new(project.name.clone());
    let is_scratch = project.source_type == "scratch";

    let draft = RwSignal::new(storage_get(session_storage(), &key).unwrap_or_default());
    let (model, effort_raw, effort) = use_model_choice(provider, key.clone());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let images = PicturesState::new(None, session.csrf_token.clone());
    let plan = RwSignal::new(false);

    Effect::new(move |_| {
        if let Some(p) = prefill_prompt.get() {
            if !p.is_empty() {
                draft.set(p);
            }
            prefill_prompt.set(None);
        }
    });

    let pending: StoredValue<RefCell<Option<(String, String)>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(None));
    let composer = NodeRef::<html::Textarea>::new();
    let controller =
        StoredValue::new_local(web_sys::AbortController::new().expect("AbortController"));
    on_cleanup(move || controller.with_value(|c| c.abort()));

    Effect::new(move |_| {
        if let Some(el) = composer.get() {
            focus_prevent_scroll(&el);
        }
    });

    Effect::new(move |_| {
        let text = draft.get();
        let k = key_sv.get_value();
        if !text.is_empty() {
            let _ = storage_set(session_storage(), &k, &text);
        } else {
            storage_remove(session_storage(), &k);
        }
        if let Some(field) = composer.get() {
            let rows: usize = text
                .split('\n')
                .map(|line| ((line.chars().count() as f64 / 75.0).ceil() as usize).max(1))
                .sum();
            field.set_rows(rows.clamp(3, 9) as u32);
        }
    });

    let add_images = Callback::new(move |files: Vec<web_sys::File>| {
        let msg = images.add(files);
        if !msg.is_empty() {
            error.set(msg);
        }
    });

    let start = Callback::new(move |execute: bool| {
        let text = draft.get_untracked();
        let cur_model = model.get_untracked();
        if text.trim().is_empty() || busy.get_untracked() || (execute && cur_model.is_empty()) {
            return;
        }
        if images.items.with_untracked(|v| !v.is_empty()) && !execute {
            error.set("Images are sent with an agent run, not saved on their own.".into());
            return;
        }
        if text.len() > AGENT_MESSAGE_MAX_BYTES {
            error.set("Keep each message within 16 KiB.".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        let pid = project_id.get_value();
        let (tid, proj_id) = pending.with_value(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.as_ref().is_none_or(|(_, p)| p != &pid) {
                *slot = Some((random_uuid(), pid.clone()));
            }
            slot.clone().unwrap()
        });
        let body = json!({ "id": tid, "projectId": proj_id }).to_string();
        let signal = controller.with_value(|c| c.signal());
        let token = csrf.get_value();
        let cur_effort = effort.get_untracked();
        let cur_plan = plan.get_untracked();
        let files: Vec<SendWrapper<web_sys::File>> = images
            .items
            .with_untracked(|list| list.iter().map(|i| i.file.clone()).collect());
        spawn_local(async move {
            let res = api::request::<AgentThread>(
                &format!("{BASE}/threads"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await;
            if signal.aborted() {
                return;
            }
            match res {
                Ok(row) => {
                    let target = format!("dar.agent.draft.{}.{}", user_id_sv.get_value(), row.id);
                    draft_cache.with_value(|c| {
                        c.borrow_mut().insert(target.clone(), text.clone());
                    });
                    let _ = storage_set(session_storage(), &target, &text);
                    if !cur_model.is_empty() {
                        let _ =
                            storage_set(session_storage(), &format!("{target}.model"), &cur_model);
                    }
                    if !cur_effort.is_empty() {
                        let _ = storage_set(
                            session_storage(),
                            &format!("{target}.effort"),
                            &cur_effort,
                        );
                    }
                    if cur_plan {
                        let _ = storage_set(session_storage(), &format!("{target}.mode"), "plan");
                    }
                    storage_remove(session_storage(), &key_sv.get_value());
                    pending.with_value(|c| *c.borrow_mut() = None);
                    {
                        on_created.try_run((row, execute, files));
                    };
                }
                Err(e) => {
                    error.try_set(failure(&e));
                    busy.try_set(false);
                }
            }
        });
    });

    let slash_commands = Signal::derive(move || {
        let cur_model = model.get();
        let efforts = provider.with(|p| {
            p.as_ref()
                .and_then(|p| p.models.iter().find(|m| m.id == cur_model))
                .and_then(|m| m.efforts.clone())
                .unwrap_or_default()
        });
        let mut list: Vec<SlashCommand> = commands
            .run(SlashControls {
                set_effort: Callback::new(move |val: String| effort_raw.set(val)),
                efforts,
            })
            .into_iter()
            .filter(|item| {
                !matches!(
                    item.name.as_str(),
                    "find" | "review" | "terminal" | "settle" | "rename"
                )
            })
            .collect();
        list.push(SlashCommand {
            name: "plan".into(),
            hint: "Toggle plan mode for the first turn".into(),
            argument: None,
            run: Callback::new(move |_| plan.update(|v| *v = !*v)),
        });
        list
    });

    let placeholder_sig = Signal::derive(move || {
        if provider.with(|p| p.as_ref().is_some_and(|p| p.connected)) {
            format!(
                "Ask {} to plan, change or explain\u{2026} / commands",
                provider_name(&model.get())
            )
        } else {
            "Write a message to save\u{2026}".into()
        }
    });

    let pname = project_name.get_value();

    view! {
        <div class="agent-draft">
            <div class="agent-hero">
                <h2>
                    "What should we build in "
                    <button type="button" class="agent-hero-project" aria-label=format!("Project: {pname}. Change project") on:click=move |_| { on_choose_project.try_run(()); }>
                        <ProjectMark name=pname.clone()/>
                        {pname.clone()}
                        <Icon name="chevron"/>
                    </button>
                    "?"
                </h2>
                {(!is_scratch).then(|| view! {
                    <p class="agent-hero-scratch">
                        <button type="button" class="text-button" on:click=move |_| { on_start_scratch.try_run(()); }>"or start without a project"</button>
                    </p>
                })}
            </div>
            <form class="agent-composer-area" on:submit=move |e| {
                e.prevent_default();
                let exec = provider.with_untracked(|p| p.as_ref().is_some_and(|p| p.connected));
                start.run(exec);
            }>
                {move || { let e = error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
                <div class=move || if plan.get() { "agent-composer is-plan" } else { "agent-composer" }
                    on:dragover=move |e| {
                        let exec = provider.with_untracked(|p| p.as_ref().is_some_and(|p| p.connected));
                        if exec && data_transfer_has_image(&e) {
                            e.prevent_default();
                        }
                    }
                    on:drop=move |e| {
                        let exec = provider.with_untracked(|p| p.as_ref().is_some_and(|p| p.connected));
                        let dropped = e.data_transfer().map(|dt| files_from_list(dt.files(), true)).unwrap_or_default();
                        if exec && !dropped.is_empty() {
                            e.prevent_default();
                            add_images.run(dropped);
                        }
                    }>
                    <PictureTray pictures=images/>
                    <PromptInput
                        input_ref=composer
                        value=draft
                        on_change=Callback::new(move |v| draft.set(v))
                        on_submit=Callback::new(move |_| {
                            let exec = provider.with_untracked(|p| p.as_ref().is_some_and(|p| p.connected));
                            start.run(exec);
                        })
                        disabled=busy
                        send_key=send_key
                        prompts=Signal::derive(|| Vec::<String>::new())
                        commands=slash_commands
                        placeholder=placeholder_sig
                        on_files=Callback::new(move |files| {
                            if provider.with_untracked(|p| p.as_ref().is_some_and(|p| p.connected)) {
                                add_images.run(files);
                            }
                        })
                    />
                    <div class="agent-composer-controls">
                        {move || provider.with(|p| p.as_ref().is_some_and(|p| p.connected)).then(|| view! {
                            <AttachButton on_files=add_images disabled=Signal::derive(move || busy.get() || images.items.with(|v| v.len() >= AGENT_ATTACHMENT_MAX_COUNT))/>
                        })}
                        {move || {
                            let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
                            let configured = provider.with(|p| p.as_ref().is_some_and(|p| p.configured));
                            if connected {
                                let models_sig = Signal::derive(move || provider.with(|p| p.as_ref().map(|p| p.models.clone()).unwrap_or_default()));
                                view! {
                                    <ModelPicker
                                        models=models_sig
                                        value=model
                                        on_change=Callback::new(move |id| model.set(id))
                                        effort=effort
                                        on_effort=Callback::new(move |v| effort_raw.set(v))
                                        disabled=busy
                                        starred=starred_models
                                        on_toggle_star=on_toggle_star
                                    />
                                }.into_any()
                            } else {
                                view! {
                                    <span class="agent-provider-state">
                                        <span class="agent-status-dot"/>
                                        {if checking_provider.get() { "Checking agents\u{2026}" } else if configured { "Agent unavailable" } else { "No provider connected" }}
                                    </span>
                                }.into_any()
                            }
                        }}
                        {move || provider.with(|p| p.as_ref().is_some_and(|p| p.connected)).then(|| view! {
                            <button type="button" class="agent-plan-toggle" aria-pressed=move || plan.get().to_string() title="Plan mode: Codex plans and asks before changing files" on:click=move |_| plan.update(|v| *v = !*v)>
                                <Icon name="list"/>"Plan"
                            </button>
                            <ActionMenu label="Message actions" actions=Callback::new(move |_| {
                                let dis = busy.get_untracked() || draft.with_untracked(|d| d.trim().is_empty());
                                vec![MenuAction::new("Save only", "file").disabled(dis).run(move || start.run(false))]
                            })><Icon name="more"/></ActionMenu>
                        })}
                        {move || {
                            let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
                            let prov = provider_name(&model.get());
                            let has_provider = provider.with(Option::is_some);
                            view! {
                                <button class="agent-send"
                                    aria-label=move || if busy.get() { "Starting\u{2026}".into() } else if connected { format!("Run with {prov}") } else { "Save message".into() }
                                    title=if connected { format!("Run with {prov} (Enter)") } else { "Save message (Enter)".into() }
                                    disabled=move || busy.get() || draft.with(|d| d.trim().is_empty()) || (connected && model.with(String::is_empty)) || (checking_provider.get() && !has_provider)>
                                    <Icon name="send"/>
                                </button>
                            }
                        }}
                    </div>
                </div>
                <div class="agent-composer-hint" role="status">{move || if busy.get() { "Starting thread\u{2026}" } else { "" }}</div>
            </form>
        </div>
    }
}

// ---------------------------------------------------------------------------
// HistoryBranchCard & ChangeReview
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct MergeResult {
    #[allow(dead_code)]
    merged: bool,
    #[serde(default)]
    conflicts: Option<Vec<String>>,
}

#[component]
fn HistoryBranchCard(
    #[prop(into)] thread_id: Signal<String>,
    #[prop(into)] completed: Signal<bool>,
    #[prop(into)] state: Signal<BranchState>,
    on_state: Callback<BranchState>,
    session: BrowserSession,
) -> impl IntoView {
    let busy = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let conflicts: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let notice = RwSignal::new(String::new());
    let confirm = RwSignal::new(false);
    let csrf = StoredValue::new(session.csrf_token);

    let commit = move || {
        busy.set("commit".into());
        error.set(String::new());
        conflicts.set(Vec::new());
        notice.set(String::new());
        let tid = thread_id.get_untracked();
        let token = csrf.get_value();
        spawn_local(async move {
            match api::request::<BranchState>(
                &format!("{BASE}/threads/{tid}/branch"),
                Req {
                    method: "POST",
                    body: Some("{}".into()),
                    csrf: Some(&token),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(next) => {
                    {
                        on_state.try_run(next);
                    };
                    notice.try_set("Changes committed to the branch.".into());
                }
                Err(e) => {
                    error.try_set(history_failure(&e.message));
                }
            }
            busy.try_set(String::new());
        });
    };

    let finish = Callback::new(move |action: &'static str| {
        let st = state.get_untracked();
        let (Some(branch), Some(folder_id)) = (st.branch, st.folder_id) else {
            return;
        };
        busy.set(action.into());
        error.set(String::new());
        conflicts.set(Vec::new());
        notice.set(String::new());
        let tid = thread_id.get_untracked();
        let token = csrf.get_value();
        let body = json!({ "name": branch.name, "revision": st.revision }).to_string();
        spawn_local(async move {
            let res = api::request::<MergeResult>(
                &format!("/api/v1/files/history/{folder_id}/branches/{action}"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    ..Default::default()
                },
            )
            .await;
            match res {
                Ok(result) => {
                    if let Some(conf) = result.conflicts.filter(|c| !c.is_empty()) {
                        conflicts.try_set(conf);
                    } else {
                        notice.try_set(if action == "merge" {
                            "Merged into the folder and committed to its history.".into()
                        } else {
                            "Branch discarded. The folder was not changed.".into()
                        });
                        if let Ok(refreshed) =
                            api::get::<BranchState>(&format!("{BASE}/threads/{tid}/branch")).await
                        {
                            {
                                on_state.try_run(refreshed);
                            };
                        }
                    }
                }
                Err(e) => {
                    error.try_set(history_failure(&e.message));
                }
            }
            busy.try_set(String::new());
            confirm.try_set(false);
        });
    });

    view! {
        <div class="agent-history-branch">
            {move || {
                let st = state.get();
                let branch = st.branch;
                let has_branch = branch.is_some();
                let heading = branch.as_ref().map(|b| b.name.clone()).unwrap_or_else(|| "Folder history".into());
                let sub = match &branch {
                    Some(b) => {
                        let count = b.changes.len();
                        let when = local_date_time(b.modified_at, &[("dateStyle", "medium"), ("timeStyle", "short")]);
                        format!("{count} {} \u{00b7} committed {when}", if count == 1 { "file" } else { "files" })
                    }
                    None => "Commit these changes to a branch, then merge them into the folder.".into(),
                };
                let b_name = branch.as_ref().map(|b| b.name.clone()).unwrap_or_default();
                view! {
                    <div class="agent-history-branch-heading">
                        <Icon name="history"/>
                        <div><strong>{heading}</strong><small>{sub}</small></div>
                    </div>
                    <div class="agent-history-branch-actions">
                        <button class=if has_branch { "secondary-button" } else { "primary-button" }
                            disabled=move || !completed.get() || !busy.with(String::is_empty)
                            title=move || (!completed.get()).then_some("Available when the run has finished")
                            on:click=move |_| commit()>
                            {move || if busy.get() == "commit" { "Committing\u{2026}" } else if has_branch { "Update branch" } else { "Commit to branch" }}
                        </button>
                        {has_branch.then(|| view! {
                            <button class="primary-button" disabled=move || !busy.with(String::is_empty) on:click=move |_| finish.run("merge")>
                                {move || if busy.get() == "merge" { "Merging\u{2026}" } else { "Merge into folder" }}
                            </button>
                            <button class="text-button danger" disabled=move || !busy.with(String::is_empty) on:click=move |_| confirm.set(true)>"Discard"</button>
                        })}
                    </div>
                    {move || {
                        let conf = conflicts.get();
                        (!conf.is_empty()).then(|| view! {
                            <div class="error" role="alert">
                                {format!("These files changed after the agent started, so nothing was merged: {}. Start a new thread from the current folder, or discard this branch.", conf.join(", "))}
                            </div>
                        })
                    }}
                    {move || { let e = error.get(); (!e.is_empty()).then(|| view! { <p class="error" role="alert">{e}</p> }) }}
                    <span class="sr-only" role="status">{move || notice.get()}</span>
                    {move || { let n = notice.get(); (!n.is_empty()).then(|| view! { <p class="agent-history-branch-notice">{n}</p> }) }}
                    {move || {
                        let b_name = b_name.clone();
                        confirm.get().then(move || {
                            let b_name = StoredValue::new(b_name.clone());
                            view! {
                                <Dialog title="Discard this branch?" on_close=Callback::new(move |_| confirm.set(false))>
                                    <p>{format!("The committed changes on {} are deleted. The folder and this thread\u{2019}s working copy are not changed.", b_name.get_value())}</p>
                                    <div class="dialog-actions">
                                        <button class="text-button" on:click=move |_| confirm.set(false)>"Cancel"</button>
                                        <button class="danger-button" disabled=move || !busy.with(String::is_empty) on:click=move |_| finish.run("discard")>"Discard branch"</button>
                                    </div>
                                </Dialog>
                            }
                        })
                    }}
                }
            }}
        </div>
    }
}

#[derive(Deserialize)]
struct UploadReservation {
    id: String,
}

#[component]
fn ChangeReview(
    #[prop(into)] run: Signal<Option<AgentRun>>,
    session: BrowserSession,
    #[prop(into)] default_diff_view: Signal<&'static str>,
    on_attach_chip: Callback<ContextChip>,
    on_reverted: Callback<()>,
) -> impl IntoView {
    let busy = RwSignal::new(String::new());
    let reverting = RwSignal::new(String::new());
    let reverted: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let error = RwSignal::new(String::new());
    let applied: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let download = RwSignal::new(String::new());
    let branch_state = RwSignal::new(BranchState::default());
    let wrap = RwSignal::new(false);
    let diff_mode = RwSignal::new(default_diff_view.get_untracked());
    let collapsed: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let notice = RwSignal::new(String::new());
    let csrf = StoredValue::new(session.csrf_token.clone());
    let session_sv = StoredValue::new(session);
    let controller =
        StoredValue::new_local(web_sys::AbortController::new().expect("AbortController"));
    on_cleanup(move || controller.with_value(|c| c.abort()));

    Effect::new(move |_| {
        diff_mode.set(default_diff_view.get());
    });

    let thread_id = Memo::new(move |_| run.with(|r| r.as_ref().map(|r| r.thread_id.clone())));
    Effect::new(move |_| {
        let Some(tid) = thread_id.get() else { return };
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        spawn_local(async move {
            if let Ok(st) = api::request::<BranchState>(
                &format!("{BASE}/threads/{tid}/branch"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
                && !signal.aborted()
            {
                branch_state.try_set(st);
            }
        });
    });

    let patches = Memo::new(move |_| {
        run.with(|r| {
            r.as_ref()
                .map(|r| {
                    r.document
                        .changes
                        .iter()
                        .map(|change| {
                            (
                                change.clone(),
                                unified_patch(&change.diff),
                                diff_counts(&change.diff),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });

    Effect::new(move |_| {
        let list = patches.get();
        if list.is_empty() {
            download.set(String::new());
            return;
        }
        let joined = list
            .iter()
            .map(|(_, p, _)| p.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let parts = js_sys::Array::of1(&JsValue::from_str(&joined));
        let bag = web_sys::BlobPropertyBag::new();
        bag.set_type("text/plain");
        if let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &bag)
            && let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob)
        {
            download.set(url.clone());
            on_cleanup(move || {
                let _ = web_sys::Url::revoke_object_url(&url);
            });
        }
    });

    let apply = move |change: AgentChange| {
        let (Some(content), Some(file_id), Some(expected_revision)) =
            (change.content, change.file_id, change.expected_revision)
        else {
            return;
        };
        let path = change.path.clone();
        busy.set(path.clone());
        error.set(String::new());
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let media_type = change.media_type.unwrap_or_else(|| "text/plain".into());
        let size = content.len();
        let signal = controller.with_value(|c| c.signal());
        let token = csrf.get_value();
        spawn_local(async move {
            let res_body = json!({
                "fileId": file_id,
                "expectedRevision": expected_revision,
                "name": name,
                "size": size,
                "mediaType": media_type,
            })
            .to_string();
            let reservation = api::request::<UploadReservation>(
                "/api/v1/files/uploads",
                Req {
                    method: "POST",
                    body: Some(res_body),
                    csrf: Some(&token),
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await;
            let result = match reservation {
                Ok(res) => {
                    let parts = js_sys::Array::of1(&JsValue::from_str(&content));
                    let bag = web_sys::BlobPropertyBag::new();
                    bag.set_type(&media_type);
                    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &bag)
                        .expect("blob");
                    api::request::<Value>(
                        &format!("/api/v1/files/uploads/{}", res.id),
                        Req {
                            method: "PUT",
                            raw: Some(blob.into()),
                            csrf: Some(&token),
                            signal: Some(signal.clone()),
                            ..Default::default()
                        },
                    )
                    .await
                    .map(|_| ())
                }
                Err(e) => Err(e),
            };
            if signal.aborted() {
                return;
            }
            match result {
                Ok(()) => {
                    applied.try_update(|v| v.push(path));
                }
                Err(e) => {
                    error.try_set(if e.status == 409 {
                    "The file changed since this workspace was created. Your proposed edit is retained; open Files to resolve the conflict.".into()
                } else {
                    failure(&e)
                });
                }
            }
            busy.try_set(String::new());
        });
    };

    let copy_diff = move |text: String| {
        spawn_local(async move {
            match write_clipboard(&text).await {
                Ok(()) => {
                    notice.try_set("Diff copied.".into());
                }
                Err(()) => {
                    error.try_set("Could not copy. Use Download review instead.".into());
                }
            }
        });
    };

    let revert_path = move |path: String| {
        let Some(tid) = thread_id.get_untracked() else {
            return;
        };
        reverting.set(path.clone());
        error.set(String::new());
        let token = csrf.get_value();
        let signal = controller.with_value(|c| c.signal());
        let body = json!({ "path": path }).to_string();
        spawn_local(async move {
            match api::request::<Value>(
                &format!("{BASE}/threads/{tid}/revert"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) if !signal.aborted() => {
                    reverted.try_update(|list| {
                        if !list.contains(&path) {
                            list.push(path.clone());
                        }
                    });
                    notice.try_set(format!("Reverted {path} in working copy."));
                    on_reverted.try_run(());
                }
                Err(e) if !signal.aborted() => {
                    error.try_set(failure(&e));
                }
                _ => {}
            }
            if !signal.aborted() {
                reverting.try_set(String::new());
            }
        });
    };

    view! {
        {move || {
            let list = patches.get();
            if list.is_empty() {
                return view! {
                    <div class="agent-review-empty">
                        <Icon name="file"/>
                        <h3>"No changes yet"</h3>
                        <p>"Files the agent edits in this thread appear here for review."</p>
                    </div>
                }.into_any();
            }
            let count = list.len();
            let (total_added, total_removed) = list.iter().fold((0usize, 0usize), |(a, r), (_, _, (da, dr))| (a + da, r + dr));
            let all_collapsed = collapsed.with(|c| c.len() == count);
            let all_paths: Vec<String> = list.iter().map(|(ch, _, _)| ch.path.clone()).collect();
            let tracked = branch_state.with(|b| b.tracked);
            let run_completed = run.with(|r| r.as_ref().is_some_and(|r| r.status == "completed"));
            let has_run = run.with(Option::is_some);
            view! {
                <div class="agent-review-toolbar">
                    <span class="agent-review-scope">
                        <strong>{format!("{count} {}", if count == 1 { "file" } else { "files" })}</strong>
                        <DiffStat added=total_added removed=total_removed/>
                        <span>"\u{00b7} Latest run"</span>
                    </span>
                    <div class="agent-diff-mode-toggle" role="radiogroup" aria-label="Diff view">
                        <button type="button" role="radio" aria-checked=move || (diff_mode.get() == "unified").to_string() on:click=move |_| diff_mode.set("unified")>"Unified"</button>
                        <button type="button" role="radio" aria-checked=move || (diff_mode.get() == "split").to_string() on:click=move |_| diff_mode.set("split")>"Split"</button>
                    </div>
                    <button class="icon-button" aria-label="Wrap lines" title="Wrap lines" aria-pressed=move || wrap.get().to_string() on:click=move |_| wrap.update(|v| *v = !*v)><Icon name="menu"/></button>
                    <button class="icon-button" aria-label=if all_collapsed { "Expand diff" } else { "Collapse diff" } title=if all_collapsed { "Expand all" } else { "Collapse all" }
                        on:click=move |_| {
                            if all_collapsed { collapsed.set(Vec::new()); } else { collapsed.set(all_paths.clone()); }
                        }>
                        <Icon name=if all_collapsed { "grid" } else { "list" }/>
                    </button>
                    {move || {
                        let url = download.get();
                        (!url.is_empty()).then(|| view! {
                            <a class="icon-button" href=url download="agent-review.diff" aria-label="Download review" title="Download review"><Icon name="download"/></a>
                        })
                    }}
                </div>
                {(tracked && has_run).then(|| {
                    let tid_sig = Signal::derive(move || run.with(|r| r.as_ref().map(|r| r.thread_id.clone()).unwrap_or_default()));
                    let comp_sig = Signal::derive(move || run.with(|r| r.as_ref().is_some_and(|r| r.status == "completed")));
                    view! { <HistoryBranchCard thread_id=tid_sig completed=comp_sig state=branch_state on_state=Callback::new(move |s| branch_state.set(s)) session=session_sv.get_value()/> }
                })}
                {list.into_iter().map(|(change, patch, (added, removed))| {
                    let path = change.path.clone();
                    let open = !collapsed.with(|c| c.contains(&path));
                    let path_toggle = path.clone();
                    let patch_copy = patch.clone();
                    let kind_label = match change.kind.as_str() { "added" => "New", "deleted" => "Deleted", _ => "Modified" };
                    let can_apply = open && !tracked && change.content.is_some() && change.file_id.is_some() && run_completed;
                    let change_apply = change.clone();
                    let path_btn = path.clone();
                    view! {
                        <section class="agent-review-file" aria-label=path.clone()>
                            <header>
                                <button class="agent-review-file-toggle" aria-expanded=open.to_string() aria-label=format!("{} {path}", if open { "Collapse" } else { "Expand" })
                                    on:click=move |_| {
                                        collapsed.update(|c| {
                                            if open { c.push(path_toggle.clone()); } else { c.retain(|p| p != &path_toggle); }
                                        });
                                    }>
                                    <Icon name="chevron"/>
                                    <span class="agent-review-path" title=path.clone()>{path.clone()}</span>
                                </button>
                                <span class=format!("agent-change-kind is-{}", change.kind)>{kind_label}</span>
                                <DiffStat added=added removed=removed/>
                                {(!reverted.with(|l| l.contains(&path))).then(|| {
                                    let path_rev = path.clone();
                                    let is_rev_busy = reverting.with(|b| b == &path);
                                    view! {
                                        <button type="button" class="icon-button" aria-label=format!("Revert {path}") title="Revert working copy file to baseline" disabled=is_rev_busy
                                            on:click=move |_| revert_path(path_rev.clone())>
                                            <Icon name="undo"/>
                                        </button>
                                    }
                                })}
                                <button class="icon-button" aria-label=format!("Copy diff for {path}") title="Copy diff" on:click=move |_| copy_diff(patch_copy.clone())><Icon name="copy"/></button>
                            </header>
                            {open.then(|| {
                                let p_split = path.clone();
                                let patch_inner = patch.clone();
                                move || {
                                    if diff_mode.get() == "split" {
                                        view! { <SplitDiffView path=p_split.clone() patch=patch_inner.clone() wrap=wrap on_comment=on_attach_chip/> }.into_any()
                                    } else {
                                        view! { <Diff source=patch_inner.clone() wrap=wrap/> }.into_any()
                                    }
                                }
                            })}
                            {can_apply.then(|| {
                                let is_applied = applied.with(|a| a.contains(&path_btn));
                                let cur_busy = busy.get();
                                view! {
                                    <div class="agent-review-apply">
                                        <span>"Edits stay in this thread\u{2019}s working copy until applied."</span>
                                        <button class="primary-button" disabled=!cur_busy.is_empty() || is_applied on:click=move |_| apply(change_apply.clone())>
                                            {if cur_busy == path_btn { "Applying\u{2026}" } else if is_applied { "Applied to Files" } else { "Apply this edit to Files" }}
                                        </button>
                                    </div>
                                }
                            })}
                        </section>
                    }
                }).collect_view()}
                <p class="agent-review-note">
                    {if tracked {
                        "Merging applies new, changed and deleted files together and commits them to the folder\u{2019}s history."
                    } else {
                        "New files, deletions and Git publishing are not applied automatically. Keep history for the folder in Files to merge every change at once."
                    }}
                </p>
                <span class="sr-only" role="status">{move || notice.get()}</span>
                {move || { let e = error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
            }.into_any()
        }}
    }
}

// ---------------------------------------------------------------------------
// GitCard
// ---------------------------------------------------------------------------

#[component]
fn GitCard(
    #[prop(into)] thread_id: Signal<String>,
    session: BrowserSession,
    #[prop(into)] epoch: Signal<u64>,
) -> impl IntoView {
    let state: RwSignal<Option<GitState>> = RwSignal::new(None);
    let dialog: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let value = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let csrf = StoredValue::new(session.csrf_token);

    Effect::new(move |_| {
        let tid = thread_id.get();
        let _ep = epoch.get();
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        spawn_local(async move {
            match api::request::<GitState>(
                &format!("{BASE}/threads/{tid}/git"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(st) => {
                    if !signal.aborted() {
                        state.try_set(Some(st));
                    }
                }
                Err(_) => {
                    if !signal.aborted() {
                        state.try_set(Some(GitState::default()));
                    }
                }
            }
        });
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(kind) = dialog.get_untracked() else {
            return;
        };
        busy.set(true);
        error.set(String::new());
        let tid = thread_id.get_untracked();
        let val = value.get_untracked();
        let body = if kind == "branch" {
            json!({ "action": "branch", "name": val }).to_string()
        } else {
            json!({ "action": "commit", "message": val }).to_string()
        };
        let token = csrf.get_value();
        spawn_local(async move {
            match api::request::<GitState>(
                &format!("{BASE}/threads/{tid}/git"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(next) => {
                    state.try_set(Some(next));
                    dialog.try_set(None);
                }
                Err(e) => {
                    error.try_set(failure(&e));
                }
            }
            busy.try_set(false);
        });
    };

    view! {
        {move || {
            let st = state.get().filter(|s| s.available)?;
            let branch = st.branch.clone().unwrap_or_default();
            let changed = st.changed.unwrap_or(0);
            Some(view! {
                <div class="agent-context-git">
                    <Icon name="share"/>
                    <span class="agent-git-branch" title=branch.clone()>{branch.clone()}</span>
                    <span>{if changed > 0 { format!("{changed} changed") } else { "Clean".into() }}</span>
                    <ActionMenu label="Git actions" actions=Callback::new(move |_| {
                        let ch = state.with_untracked(|s| s.as_ref().and_then(|s| s.changed).unwrap_or(0));
                        vec![
                            MenuAction::new("New branch\u{2026}", "plus").run(move || { value.set(String::new()); error.set(String::new()); dialog.set(Some("branch")); }),
                            MenuAction::new("Commit changes\u{2026}", "check").disabled(ch == 0).run(move || { value.set(String::new()); error.set(String::new()); dialog.set(Some("commit")); }),
                        ]
                    })><Icon name="more"/></ActionMenu>
                </div>
                {move || dialog.get().map(|kind| {
                    let is_branch = kind == "branch";
                    let title = if is_branch { "New branch" } else { "Commit changes" };
                    let b_sv = StoredValue::new(branch.clone());
                    view! {
                        <Dialog title=title on_close=Callback::new(move |_| if !busy.get_untracked() { dialog.set(None); })>
                            <form class="agent-git-form" on:submit=submit>
                                <label>
                                    {if is_branch { "Branch name" } else { "Commit message" }}
                                    {if is_branch {
                                        view! { <input autofocus required prop:value=move || value.get() maxlength="100" spellcheck="false" placeholder="feature/greeting" on:input=move |e| value.set(event_target_value(&e))/> }.into_any()
                                    } else {
                                        view! { <textarea autofocus required prop:value=move || value.get() maxlength="500" rows="3" on:input=move |e| value.set(event_target_value(&e))/> }.into_any()
                                    }}
                                </label>
                                <p class="muted small">
                                    {if is_branch {
                                        format!("Created from {} in this thread\u{2019}s working copy. Pushing and pull requests are disabled: Dar workspaces have no network access or Git credentials.", b_sv.get_value())
                                    } else {
                                        format!("Commits {changed} changed {} on {}. Pushing and pull requests are disabled: Dar workspaces have no network access or Git credentials.", if changed == 1 { "file" } else { "files" }, b_sv.get_value())
                                    }}
                                </p>
                                {move || { let e = error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
                                <div class="dialog-actions">
                                    <button type="button" class="secondary-button" disabled=move || busy.get() on:click=move |_| dialog.set(None)>"Cancel"</button>
                                    <button class="primary-button" disabled=move || busy.get() || value.with(|v| v.trim().is_empty())>
                                        {move || if busy.get() { "Working\u{2026}" } else if is_branch { "Create branch" } else { "Commit" }}
                                    </button>
                                </div>
                            </form>
                        </Dialog>
                    }
                })}
            })
        }}
    }
}

// ---------------------------------------------------------------------------
// ShareDialog & SharedConversation
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ShareList {
    items: Vec<ShareRow>,
}

#[component]
fn ShareDialog(
    thread: AgentThread,
    session: BrowserSession,
    on_close: Callback<()>,
) -> impl IntoView {
    let rows: RwSignal<Option<Vec<ShareRow>>> = RwSignal::new(None);
    let username = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let operation = StoredValue::new_local(RefCell::new(random_uuid()));
    let thread_id = StoredValue::new(thread.id);
    let csrf = StoredValue::new(session.csrf_token);

    Effect::new(move |_| {
        let tid = thread_id.get_value();
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        spawn_local(async move {
            match api::request::<ShareList>(
                &format!("{BASE}/threads/{tid}/shares"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(val) => {
                    if !signal.aborted() {
                        rows.try_set(Some(val.items));
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        error.try_set(failure(&e));
                    }
                }
            }
        });
    });

    let add = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        busy.set(true);
        error.set(String::new());
        let tid = thread_id.get_value();
        let op_id = operation.with_value(|c| c.borrow().clone());
        let uname = username.get_untracked().trim().to_owned();
        let body = json!({ "id": op_id, "username": uname }).to_string();
        let token = csrf.get_value();
        spawn_local(async move {
            match api::request::<ShareRow>(
                &format!("{BASE}/threads/{tid}/shares"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(row) => {
                    rows.try_update(|list| {
                        let mut next = list.take().unwrap_or_default();
                        next.retain(|item| item.id != row.id);
                        next.push(row);
                        *list = Some(next);
                    });
                    username.try_set(String::new());
                    operation.with_value(|c| *c.borrow_mut() = random_uuid());
                }
                Err(e) => {
                    error.try_set(match e.status {
                        404 => "No active Dar account has that username.".into(),
                        400 => "Enter another person\u{2019}s username.".into(),
                        _ => failure(&e),
                    });
                }
            }
            busy.try_set(false);
        });
    };

    let remove = Callback::new(move |row_id: String| {
        let tid = thread_id.get_value();
        let token = csrf.get_value();
        spawn_local(async move {
            match api::request::<Value>(
                &format!("{BASE}/threads/{tid}/shares/{row_id}"),
                Req {
                    method: "DELETE",
                    csrf: Some(&token),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    rows.try_update(|list| {
                        if let Some(v) = list {
                            v.retain(|item| item.id != row_id);
                        }
                    });
                }
                Err(e) => {
                    error.try_set(failure(&e));
                }
            }
        });
    });

    view! {
        <Dialog title="Share thread" on_close=on_close>
            <div class="agent-share">
                <p class="muted small">"People you add can read this thread\u{2019}s messages. They cannot send messages, see run output or open the working copy."</p>
                <form on:submit=add>
                    <label class="sr-only" for="agent-share-user">"Username"</label>
                    <input id="agent-share-user" autofocus autocapitalize="none" spellcheck="false" placeholder="Dar username" prop:value=move || username.get() maxlength="100"
                        on:input=move |e| username.set(event_target_value(&e))/>
                    <button class="primary-button" disabled=move || busy.get() || username.with(|u| u.trim().is_empty())>
                        {move || if busy.get() { "Sharing\u{2026}" } else { "Share" }}
                    </button>
                </form>
                {move || { let e = error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
                {move || match rows.get() {
                    None => view! { <p role="status" class="muted small">"Loading\u{2026}"</p> }.into_any(),
                    Some(list) if list.is_empty() => view! { <p class="muted small">"Only you can see this thread."</p> }.into_any(),
                    Some(list) => view! {
                        <ul aria-label="People with access">
                            {list.into_iter().map(|row| {
                                let name = if row.display_name.is_empty() { row.username.clone() } else { row.display_name.clone() };
                                let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                                let rid = row.id.clone();
                                view! {
                                    <li>
                                        <span class="avatar" aria-hidden="true">{initial}</span>
                                        <span><strong>{name}</strong><small>{format!("{} \u{00b7} read only", row.username)}</small></span>
                                        <button class="text-button" on:click=move |_| remove.run(rid.clone())>"Remove"</button>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any(),
                }}
            </div>
        </Dialog>
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedMessages {
    items: Vec<AgentMessage>,
    next_before: Option<u64>,
}

#[component]
fn SharedConversation(#[prop(into)] item: Signal<SharedThread>) -> impl IntoView {
    let items: RwSignal<Option<Vec<AgentMessage>>> = RwSignal::new(None);
    let next: RwSignal<Option<u64>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let loading = RwSignal::new(false);

    // As in React, only the thread and its revision reload the messages.
    let key = Memo::new(move |_| item.with(|i| (i.thread.id.clone(), i.thread.revision)));
    Effect::new(move |_| {
        let (tid, _rev) = key.get();
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        items.set(None);
        error.set(String::new());
        spawn_local(async move {
            match api::request::<SharedMessages>(
                &format!("{BASE}/shared/{tid}/messages"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(val) => {
                    if !signal.aborted() {
                        items.try_set(Some(val.items));
                        next.try_set(val.next_before);
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        error.try_set(failure(&e));
                    }
                }
            }
        });
    });

    let earlier = move || {
        let Some(before) = next.get_untracked() else {
            return;
        };
        let tid = item.with_untracked(|i| i.thread.id.clone());
        loading.set(true);
        spawn_local(async move {
            match api::get::<SharedMessages>(&format!(
                "{BASE}/shared/{tid}/messages?before={before}"
            ))
            .await
            {
                Ok(val) => {
                    items.try_update(|cur| {
                        let existing = cur.as_deref().unwrap_or(&[]);
                        *cur = Some(merge_messages(existing, &val.items));
                    });
                    next.try_set(val.next_before);
                }
                Err(e) => {
                    error.try_set(failure(&e));
                }
            }
            loading.try_set(false);
        });
    };

    view! {
        <div class="agent-shared-banner" role="note">
            <Icon name="lock"/>"Shared by "<strong>{move || item.with(|i| i.owner.clone())}</strong>" \u{00b7} read only"
        </div>
        <div class="agent-timeline-wrap">
            <div class="agent-conversation" aria-label="Conversation messages" aria-busy=move || (items.with(Option::is_none) && error.with(String::is_empty)).to_string()>
                <div class="agent-timeline-content">
                    {move || { let e = error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
                    {move || (items.with(Option::is_none) && error.with(String::is_empty)).then(|| view! { <p role="status">"Loading conversation\u{2026}"</p> })}
                    {move || next.with(Option::is_some).then(|| view! {
                        <button class="text-button" disabled=move || loading.get() on:click=move |_| earlier()>
                            {move || if loading.get() { "Loading\u{2026}" } else { "Load earlier messages" }}
                        </button>
                    })}
                    {move || items.get().map(|list| {
                        if list.is_empty() {
                            view! { <p class="muted">"No messages yet."</p> }.into_any()
                        } else {
                            list.into_iter().map(|message| {
                                let time_iso = iso_string(message.created_at);
                                let time_disp = local_date_time(message.created_at, &[("month", "short"), ("day", "numeric"), ("hour", "numeric"), ("minute", "2-digit")]);
                                view! {
                                    <article data-message-id=message.id class=format!("agent-message agent-message-{}", message.role)>
                                        <div class="agent-message-body">
                                            {if message.role == "assistant" {
                                                view! { <Markdown source=message.content/> }.into_any()
                                            } else {
                                                view! { <p>{message.content}</p> }.into_any()
                                            }}
                                        </div>
                                        <div class="agent-message-meta"><time datetime=time_iso>{time_disp}</time></div>
                                    </article>
                                }
                            }).collect_view().into_any()
                        }
                    })}
                </div>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// CommandPalette & Resizer
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct PaletteItem {
    group: &'static str,
    label: String,
    detail: Option<String>,
    mark: Option<String>,
    icon: Option<&'static str>,
    run: Callback<()>,
}

#[component]
fn CommandPalette(items: Vec<PaletteItem>, on_close: Callback<()>) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let active = RwSignal::new(0usize);
    let list_ref = NodeRef::<html::Div>::new();
    let items_sv = StoredValue::new(items);

    let shown = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        items_sv.with_value(|all| {
            all.iter()
                .enumerate()
                .filter(|(_, item)| {
                    let hay = format!(
                        "{} {} {}",
                        item.label,
                        item.detail.as_deref().unwrap_or(""),
                        item.group
                    )
                    .to_lowercase();
                    words.iter().all(|w| hay.contains(w))
                })
                .take(60)
                .map(|(idx, _)| idx)
                .collect::<Vec<usize>>()
        })
    });

    Effect::new(move |_| {
        query.track();
        active.set(0);
    });

    Effect::new(move |_| {
        active.track();
        if let Some(el) = list_ref.get()
            && let Ok(Some(node)) = el.query_selector("[data-active]")
        {
            scroll_into_view_block(&node, web_sys::ScrollLogicalPosition::Nearest);
        }
    });

    // The items belong to the palette's owner, which closing disposes: run first, then close.
    let choose = move |run_cb: Callback<()>| {
        run_cb.run(());
        {
            on_close.try_run(());
        };
    };

    view! {
        <Dialog title="Command palette" on_close=on_close>
            <div class="agent-palette">
                <input autofocus type="search" role="combobox" aria-expanded="true" aria-controls="agent-palette-list"
                    aria-activedescendant=move || {
                        let cur = active.get();
                        (cur < shown.with(Vec::len)).then(|| format!("agent-palette-{cur}"))
                    }
                    aria-label="Search commands, threads and projects"
                    placeholder="Search commands, threads and projects\u{2026}"
                    prop:value=move || query.get()
                    on:input=move |e| query.set(event_target_value(&e))
                    on:keydown=move |e| {
                        let key = e.key();
                        if key == "ArrowDown" || key == "ArrowUp" {
                            e.prevent_default();
                            let len = shown.with_untracked(Vec::len);
                            if len > 0 {
                                let delta = if key == "ArrowDown" { 1 } else { len - 1 };
                                active.update(|idx| *idx = (*idx + delta) % len);
                            } else {
                                active.set(0);
                            }
                        } else if key == "Enter" {
                            let chosen = shown.with_untracked(|s| {
                                s.get(active.get_untracked()).copied().and_then(|idx| {
                                    items_sv.with_value(|all| all.get(idx).map(|i| i.run))
                                })
                            });
                            if let Some(run_cb) = chosen {
                                e.prevent_default();
                                choose(run_cb);
                            }
                        }
                    }/>
                <div id="agent-palette-list" role="listbox" aria-label="Results" node_ref=list_ref>
                    {move || {
                        let indices = shown.get();
                        let cur = active.get();
                        if indices.is_empty() {
                            return view! { <p class="muted">"No results"</p> }.into_any();
                        }
                        let list: Vec<PaletteItem> = items_sv.with_value(|all| {
                            indices.iter().filter_map(|&idx| all.get(idx).cloned()).collect()
                        });
                        list.iter().enumerate().map(|(index, item)| {
                            let show_group = index == 0 || list[index - 1].group != item.group;
                            let is_act = index == cur;
                            let run_cb = item.run;
                            let mark = item.mark.clone();
                            let icon = item.icon.unwrap_or("chevron");
                            let detail = item.detail.clone();
                            view! {
                                {show_group.then(|| view! { <div class="agent-palette-group" role="presentation">{item.group}</div> })}
                                <div id=format!("agent-palette-{index}") role="option" aria-selected=is_act.to_string() data-active=is_act.then_some("true")
                                    on:pointerenter=move |_| active.set(index)
                                    on:click=move |_| choose(run_cb)>
                                    {if let Some(m) = mark {
                                        view! { <ProjectMark name=m/> }.into_any()
                                    } else {
                                        view! { <Icon name=icon/> }.into_any()
                                    }}
                                    <span>{item.label.clone()}</span>
                                    {detail.map(|d| view! { <small>{d}</small> })}
                                </div>
                            }
                        }).collect_view().into_any()
                    }}
                </div>
            </div>
        </Dialog>
    }
}

#[component]
fn Resizer(
    label: &'static str,
    #[prop(into)] value: Signal<f64>,
    min: f64,
    max: f64,
    on_change: Callback<f64>,
    side: &'static str,
) -> impl IntoView {
    let drag_listeners: StoredValue<RefCell<Vec<Listener>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(Vec::new()));
    on_cleanup(move || drag_listeners.with_value(|v| v.borrow_mut().clear()));

    view! {
        <div class=format!("agent-resizer agent-resizer-{side}") role="separator" aria-orientation="vertical" aria-label=label
            aria-valuenow=move || (value.get().round() as i64).to_string()
            aria-valuemin=(min as i64).to_string()
            aria-valuemax=(max as i64).to_string()
            tabindex="0"
            on:keydown=move |e| {
                let key = e.key();
                if key == "ArrowLeft" || key == "ArrowRight" {
                    e.prevent_default();
                    let step = if (key == "ArrowRight") == (side == "right") { 16.0 } else { -16.0 };
                    { on_change.try_run((value.get_untracked() + step).clamp(min, max)); };
                }
            }
            on:dblclick=move |_| {
                let reset = if side == "right" {
                    256.0
                } else {
                    let iw = window().inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1400.0);
                    (iw * 0.4).round()
                };
                { on_change.try_run(reset); };
            }
            on:pointerdown=move |e: web_sys::PointerEvent| {
                e.prevent_default();
                let start = e.client_x() as f64;
                let initial = value.get_untracked();
                let Some(target) = e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
                let _ = target.set_pointer_capture(e.pointer_id());
                let dir = if side == "right" { 1.0 } else { -1.0 };
                let move_l = Listener::new(&target, "pointermove", false, move |ev| {
                    if let Ok(pe) = ev.dyn_into::<web_sys::PointerEvent>() {
                        { on_change.try_run((initial + dir * ((pe.client_x() as f64) - start)).clamp(min, max)); };
                    }
                });
                let up_l = Listener::new(&target, "pointerup", false, move |_| {
                    drag_listeners.with_value(|v| v.borrow_mut().clear());
                });
                let cancel_l = Listener::new(&target, "pointercancel", false, move |_| {
                    drag_listeners.with_value(|v| v.borrow_mut().clear());
                });
                drag_listeners.with_value(|v| {
                    let mut list = v.borrow_mut();
                    list.clear();
                    list.push(move_l);
                    list.push(up_l);
                    list.push(cancel_l);
                });
            }/>
    }
}

// ---------------------------------------------------------------------------
// AgentWorkspace
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ProjectList {
    items: Vec<AgentProject>,
}

#[derive(Deserialize)]
struct ThreadList {
    items: Vec<AgentThread>,
}

#[derive(Deserialize)]
struct SharedList {
    items: Vec<SharedThread>,
}

fn dispatch_agent_special(thread_id: &str, mode: &str) {
    let detail = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &detail,
        &JsValue::from_str("thread"),
        &JsValue::from_str(thread_id),
    );
    let _ = js_sys::Reflect::set(
        &detail,
        &JsValue::from_str("mode"),
        &JsValue::from_str(mode),
    );
    let init = web_sys::CustomEventInit::new();
    init.set_detail(&detail);
    if let Ok(ev) = web_sys::CustomEvent::new_with_event_init_dict("dar:agent-special", &init) {
        let _ = window().dispatch_event(&ev);
    }
}

fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '_' | ' ' | '.' | '-'))
        .collect();
    let trimmed = cleaned.trim();
    let cut: String = trimmed.chars().take(80).collect();
    if cut.is_empty() { "thread".into() } else { cut }
}

fn local_datetime_min_now() -> String {
    let now_ms = js_sys::Date::now();
    let offset_ms = js_sys::Date::new_0().get_timezone_offset() * 60000.0;
    let iso = iso_string(now_ms - offset_ms);
    iso.chars().take(16).collect()
}

fn parse_datetime_local(value: &str) -> f64 {
    if value.is_empty() {
        return f64::NAN;
    }
    js_sys::Date::new(&JsValue::from_str(value)).get_time()
}

#[component]
pub fn AgentWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    use_body_class(&["has-agent-shell"]);

    let initial_search = window().location().search().unwrap_or_default();
    let initial_params = web_sys::UrlSearchParams::new_with_str(&initial_search).ok();
    let initial_project = initial_params
        .as_ref()
        .and_then(|p| p.get("project"))
        .unwrap_or_default();
    let initial_thread = initial_params
        .as_ref()
        .and_then(|p| p.get("thread"))
        .unwrap_or_default();
    let has_initial_thread = initial_params.as_ref().is_some_and(|p| p.has("thread"));

    let init_layout = saved_layout(&session.user.id);
    let session_sv = StoredValue::new(session.clone());
    let user_id_sv = StoredValue::new(session.user.id.clone());
    let csrf_sv = StoredValue::new(session.csrf_token.clone());
    let is_admin = session.user.administrator;

    let projects: RwSignal<Option<Vec<AgentProject>>> = RwSignal::new(None);
    let project_id = RwSignal::new(initial_project);
    let threads: RwSignal<Option<Vec<AgentThread>>> = RwSignal::new(None);
    let thread_id = RwSignal::new(initial_thread);
    let error = RwSignal::new(String::new());
    let thread_error = RwSignal::new(String::new());
    let reload = RwSignal::new(0u64);
    let view = RwSignal::new(if is_narrow() && !has_initial_thread {
        View::Threads
    } else {
        View::Conversation
    });
    let active_page = RwSignal::new(AgentPage::Workspace);
    let active_surface = RwSignal::new(SurfaceKind::Diff);
    let surface_file: RwSignal<Option<String>> = RwSignal::new(None);
    let preview_file: RwSignal<Option<String>> = RwSignal::new(None);
    let sidebar_filter = RwSignal::new("all"); // "all" | "working" | "unread" | "pinned"
    let context_chips: RwSignal<Vec<ContextChip>> = RwSignal::new(Vec::new());
    let incoming_draft: RwSignal<Option<String>> = RwSignal::new(None);
    let prefill_draft: RwSignal<Option<String>> = RwSignal::new(None);
    let script_launch: RwSignal<Option<ScriptLaunch>> = RwSignal::new(None);
    let script_seq = RwSignal::new(0u64);
    let editor: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let settings = RwSignal::new(false);
    let settings_section: RwSignal<&'static str> = RwSignal::new("General");
    let settings_notice: RwSignal<Option<Notice>> = RwSignal::new(None);
    let review = RwSignal::new(init_layout.review);
    let nav = RwSignal::new(init_layout.nav);
    let details = RwSignal::new(init_layout.details);
    let terminal = RwSignal::new(init_layout.terminal);
    let query = RwSignal::new(String::new());
    let find_open = RwSignal::new(false);
    let maximized = RwSignal::new(false);
    let layout_error = RwSignal::new(String::new());

    let shared: RwSignal<Vec<SharedThread>> = RwSignal::new(Vec::new());
    let shared_id = RwSignal::new(String::new());
    let sharing: RwSignal<Option<AgentThread>> = RwSignal::new(None);
    let snoozing: RwSignal<Option<AgentThread>> = RwSignal::new(None);
    let snooze_at = RwSignal::new(String::new());
    let limits: RwSignal<Option<AgentLimits>> = RwSignal::new(None);
    let drawer_tab = RwSignal::new("output");
    let sidebar_width = RwSignal::new(init_layout.sidebar_width);
    let review_width = RwSignal::new(init_layout.review_width);
    let palette = RwSignal::new(false);
    let renaming = RwSignal::new(String::new());
    let rename_value = RwSignal::new(String::new());
    let deleting: RwSignal<Option<AgentThread>> = RwSignal::new(None);
    let delete_busy = RwSignal::new(false);
    let delete_error = RwSignal::new(String::new());
    let collapsed_groups: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let permission = RwSignal::new(notification_permission());

    let shell_ref = NodeRef::<html::Div>::new();
    let panel_ref = NodeRef::<html::Div>::new();
    let tabs_ref = NodeRef::<html::Nav>::new();
    let search_ref = NodeRef::<html::Input>::new();

    let statuses: StoredValue<RefCell<HashMap<String, String>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(HashMap::new()));
    let provider: RwSignal<Option<AgentProvider>> = RwSignal::new(None);
    let provider_epoch = RwSignal::new(0u64);
    let checking_provider = RwSignal::new(true);
    let connection_error = RwSignal::new(String::new());
    let run_id = RwSignal::new(String::new());
    let stored_run: RwSignal<Option<AgentRun>> = RwSignal::new(None);
    let run_error = RwSignal::new(String::new());
    let history_epoch = RwSignal::new(0u64);
    let create_error = RwSignal::new(String::new());
    let settling = RwSignal::new(false);

    let prefs_shared = shared::context().preferences;
    let prefs_loaded = Signal::derive(move || prefs_shared.value.with(Option::is_some));
    let prefs_error = Signal::derive(move || prefs_shared.error.get());

    let completed_seen: StoredValue<RefCell<HashSet<String>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(HashSet::new()));
    let auto_start: StoredValue<RefCell<HashMap<String, bool>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(HashMap::new()));
    let start_files: StoredValue<
        RefCell<HashMap<String, Vec<SendWrapper<web_sys::File>>>>,
        LocalStorage,
    > = StoredValue::new_local(RefCell::new(HashMap::new()));
    let lifetime =
        StoredValue::new_local(web_sys::AbortController::new().expect("AbortController"));
    on_cleanup(move || lifetime.with_value(|c| c.abort()));

    let now = use_now(Signal::derive(|| true), 30000);
    let previous_view = StoredValue::new_local(RefCell::new(view.get_untracked()));
    let return_to_tabs = StoredValue::new_local(RefCell::new(false));
    let draft_cache: StoredValue<RefCell<HashMap<String, String>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(HashMap::new()));
    let positions: StoredValue<RefCell<HashMap<String, ScrollPosition>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(HashMap::new()));

    let thread = Memo::new(move |_| {
        let tid = thread_id.get();
        if tid.is_empty() {
            return None;
        }
        threads.with(|list| {
            list.as_ref()
                .and_then(|v| v.iter().find(|t| t.id == tid).cloned())
        })
    });

    let project = Memo::new(move |_| {
        let t = thread.get();
        let pid = t
            .as_ref()
            .map(|t| t.project_id.clone())
            .unwrap_or_else(|| project_id.get());
        projects.with(|list| {
            let list = list.as_ref()?;
            list.iter().find(|p| p.id == pid).cloned().or_else(|| {
                if t.is_none() {
                    list.first().cloned()
                } else {
                    None
                }
            })
        })
    });

    let run = Memo::new(move |_| {
        let tid = thread_id.get();
        stored_run.with(|r| r.as_ref().filter(|r| r.thread_id == tid).cloned())
    });

    // Dynamic panel widths as CSS custom properties.
    Effect::new(move |_| {
        let sw = sidebar_width.get();
        let rw = review_width.get();
        if let Some(el) = shell_ref.get() {
            let _ = web_sys::HtmlElement::style(&el)
                .set_property("--agent-sidebar-width", &format!("{sw}px"));
            let _ = web_sys::HtmlElement::style(&el)
                .set_property("--agent-review-width", &format!("{rw}px"));
        }
    });

    // Save layout in localStorage.
    Effect::new(move |_| {
        let layout_val = Layout {
            nav: nav.get(),
            details: details.get(),
            review: review.get(),
            terminal: terminal.get(),
            sidebar_width: sidebar_width.get(),
            review_width: review_width.get(),
        };
        let key = format!("dar.agent.layout.{}", user_id_sv.get_value());
        let saved = serde_json::to_string(&layout_val)
            .ok()
            .and_then(|s| storage_set(local_storage(), &key, &s).ok());
        if saved.is_some() {
            layout_error.set(String::new());
        } else {
            layout_error.set("Layout changes could not be saved in this browser.".into());
        }
    });

    let choose_new_thread = Callback::new(move |()| {
        let Some(proj_list) = projects.get_untracked() else {
            return;
        };
        create_error.set(String::new());
        if proj_list.is_empty() {
            editor.set(Some("project"));
            return;
        }
        let target_pid = project
            .get_untracked()
            .map(|p| p.id)
            .unwrap_or_else(|| proj_list[0].id.clone());
        thread_id.set(String::new());
        project_id.set(target_pid);
        find_open.set(false);
        view.set(View::Conversation);
        request_animation_frame(|| {
            if let Some(el) = document()
                .get_element_by_id("agent-message")
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            {
                let _ = el.focus();
            }
        });
    });

    // Global keyboard shortcuts.
    listen(&document(), "keydown", false, move |event| {
        let Ok(ke) = event.dyn_into::<web_sys::KeyboardEvent>() else {
            return;
        };
        if !(ke.ctrl_key() || ke.meta_key()) || ke.default_prevented() {
            return;
        }
        if document()
            .query_selector("dialog[open],[data-editor-pane]")
            .ok()
            .flatten()
            .is_some()
        {
            return;
        }
        let key = ke.key().to_lowercase();
        if key == "p" && !ke.shift_key() {
            ke.prevent_default();
            palette.set(true);
        } else if key == "o" && ke.shift_key() {
            ke.prevent_default();
            choose_new_thread.run(());
        } else if key == "b" && !ke.shift_key() {
            ke.prevent_default();
            nav.update(|v| *v = !*v);
        } else if key == "f" && !thread_id.with_untracked(String::is_empty) {
            ke.prevent_default();
            view.set(View::Conversation);
            find_open.set(true);
        }
    });

    // Fetch provider status.
    Effect::new(move |_| {
        let _ep = provider_epoch.get();
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        checking_provider.set(true);
        connection_error.set(String::new());
        spawn_local(async move {
            match api::request::<AgentProvider>(
                &format!("{BASE}/provider"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(val) => {
                    if !signal.aborted() {
                        provider.try_set(Some(val));
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        connection_error.try_set(failure(&e));
                    }
                }
            }
            if !signal.aborted() {
                checking_provider.try_set(false);
            }
        });
    });

    let connect_provider = Callback::new(move |which: String| {
        checking_provider.set(true);
        connection_error.set(String::new());
        let signal = lifetime.with_value(|c| c.signal());
        let token = csrf_sv.get_value();
        let body = json!({ "provider": which }).to_string();
        spawn_local(async move {
            match api::request::<AgentProvider>(
                &format!("{BASE}/provider/connect"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(val) => {
                    if !signal.aborted() {
                        provider.try_set(Some(val));
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        connection_error.try_set(if e.message.is_empty() {
                            "The agent could not be connected.".into()
                        } else {
                            e.message
                        });
                    }
                }
            }
            if !signal.aborted() {
                checking_provider.try_set(false);
            }
        });
    });

    // Clear run state when thread changes.
    Effect::new(move |_| {
        let _tid = thread_id.get();
        run_id.set(String::new());
        stored_run.set(None);
        run_error.set(String::new());
    });

    // Poll active run with ETag / If-None-Match.
    Effect::new(move |_| {
        let rid = run_id.get();
        let tid = thread_id.get();
        if rid.is_empty() {
            return;
        }
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());

        spawn_local(async move {
            let mut tag = String::new();
            while !signal.aborted() {
                let headers: Vec<(&str, &str)> = if tag.is_empty() {
                    Vec::new()
                } else {
                    vec![("if-none-match", tag.as_str())]
                };
                let fetch_res = api::fetch(
                    &format!("{BASE}/runs/{rid}"),
                    Req {
                        headers: &headers,
                        signal: Some(signal.clone()),
                        ..Default::default()
                    },
                )
                .await;
                if signal.aborted() {
                    return;
                }
                let delay = match fetch_res {
                    Ok(response) => {
                        let status = response.status();
                        if status == 304 && !tag.is_empty() {
                            run_error.try_set(String::new());
                            1000
                        } else {
                            let body_json: Option<Value> = match response.json() {
                                Ok(p) => JsFuture::from(p).await.ok().and_then(|v| {
                                    let txt = js_sys::JSON::stringify(&v).ok()?.as_string()?;
                                    serde_json::from_str(&txt).ok()
                                }),
                                Err(_) => None,
                            };
                            if signal.aborted() {
                                return;
                            }
                            if !response.ok() || body_json.is_none() {
                                let msg = body_json
                                    .as_ref()
                                    .and_then(|v| v["error"]["message"].as_str())
                                    .unwrap_or("The request could not be completed.")
                                    .to_owned();
                                let err = RequestError {
                                    status,
                                    message: msg,
                                };
                                run_error.try_set(failure(&err));
                                3000
                            } else if let Ok(value) =
                                serde_json::from_value::<AgentRun>(body_json.unwrap())
                            {
                                tag = response
                                    .headers()
                                    .get("etag")
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default();
                                if value.thread_id != tid {
                                    return;
                                }
                                let active = active_agent_run(&value.status);
                                let status_str = value.status.clone();
                                let val_id = value.id.clone();
                                stored_run.try_set(Some(value));
                                run_error.try_set(String::new());
                                threads.try_update(|cur| {
                                    if let Some(list) = cur
                                        && let Some(item) = list.iter_mut().find(|i| i.id == tid)
                                        && item.run_status.as_deref() != Some(status_str.as_str())
                                    {
                                        item.run_status = Some(status_str);
                                    }
                                });
                                if active {
                                    1000
                                } else {
                                    let newly_completed = completed_seen
                                        .with_value(|set| set.borrow_mut().insert(val_id));
                                    if newly_completed {
                                        history_epoch.try_update(|n| *n += 1);
                                        reload.try_update(|n| *n += 1);
                                    }
                                    return;
                                }
                            } else {
                                run_error.try_set("The request could not be completed.".into());
                                3000
                            }
                        }
                    }
                    Err(e) => {
                        run_error.try_set(failure(&e));
                        3000
                    }
                };
                crate::sleep(delay).await;
            }
        });
    });

    // Fetch projects.
    Effect::new(move |_| {
        let _r = reload.get();
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        error.set(String::new());
        spawn_local(async move {
            match api::request::<ProjectList>(
                &format!("{BASE}/projects"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(val) => {
                    if !signal.aborted() {
                        projects.try_set(Some(val.items));
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        error.try_set(failure(&e));
                    }
                }
            }
        });
    });

    // Poll threads + refresh on window focus.
    Effect::new(move |_| {
        let _r = reload.get();
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let timer: Rc<RefCell<Option<(i32, Closure<dyn FnMut()>)>>> = Rc::new(RefCell::new(None));

        fn schedule_refresh(
            signal: web_sys::AbortSignal,
            timer: Rc<RefCell<Option<(i32, Closure<dyn FnMut()>)>>>,
            threads: RwSignal<Option<Vec<AgentThread>>>,
            thread_error: RwSignal<String>,
            thread_id: RwSignal<String>,
        ) {
            clear_timeout(&mut timer.borrow_mut());
            let sig = signal.clone();
            let t_cl = timer.clone();
            spawn_local(async move {
                let res = api::request::<ThreadList>(
                    &format!("{BASE}/threads"),
                    Req {
                        signal: Some(sig.clone()),
                        ..Default::default()
                    },
                )
                .await;
                if sig.aborted() {
                    return;
                }
                let delay = match res {
                    Ok(value) => {
                        let any_active = value
                            .items
                            .iter()
                            .any(|i| active_agent_run(i.run_status.as_deref().unwrap_or("")));
                        let cur_tid = thread_id.get_untracked();
                        if !cur_tid.is_empty() && !value.items.iter().any(|t| t.id == cur_tid) {
                            thread_id.try_set(String::new());
                        }
                        threads.try_set(Some(value.items));
                        thread_error.try_set(String::new());
                        if any_active { 3000 } else { 15000 }
                    }
                    Err(e) => {
                        thread_error.try_set(failure(&e));
                        5000
                    }
                };
                if !sig.aborted() {
                    let sig2 = sig.clone();
                    let t2 = t_cl.clone();
                    let next = set_timeout(delay, move || {
                        schedule_refresh(sig2, t2, threads, thread_error, thread_id)
                    });
                    *t_cl.borrow_mut() = next;
                }
            });
        }

        schedule_refresh(
            signal.clone(),
            timer.clone(),
            threads,
            thread_error,
            thread_id,
        );
        let sig_f = signal.clone();
        let timer_f = timer.clone();
        let focus_listener = Listener::new(&window(), "focus", false, move |_| {
            schedule_refresh(
                sig_f.clone(),
                timer_f.clone(),
                threads,
                thread_error,
                thread_id,
            );
        });
        let kept = SendWrapper::new((abort, timer, focus_listener));
        on_cleanup(move || {
            let (abort, timer, _fl) = kept.take();
            abort.abort();
            clear_timeout(&mut timer.borrow_mut());
        });
    });

    // Keep URL search params synced (?project=...&thread=...).
    Effect::new(move |_| {
        let tid = thread_id.get();
        let selected_project = thread
            .with(|t| t.as_ref().map(|t| t.project_id.clone()))
            .unwrap_or_else(|| project_id.get());
        if let Ok(href) = window().location().href()
            && let Ok(url) = web_sys::Url::new(&href)
        {
            let sp = url.search_params();
            if !selected_project.is_empty() {
                sp.set("project", &selected_project);
            } else {
                sp.delete("project");
            }
            if !tid.is_empty() {
                sp.set("thread", &tid);
            } else {
                sp.delete("thread");
            }
            if let Ok(hist) = window().history() {
                let _ = hist.replace_state_with_url(&JsValue::NULL, "", Some(&url.href()));
            }
        }
    });

    // Mobile panel focus management when view changes.
    Effect::new(move |_| {
        let cur = view.get();
        let prev = previous_view.with_value(|p| *p.borrow());
        if prev != cur && is_narrow() {
            let to_tabs = return_to_tabs.with_value(|r| *r.borrow());
            if to_tabs {
                if let Some(el) = tabs_ref
                    .get_untracked()
                    .and_then(|t| t.query_selector("button[aria-current=page]").ok().flatten())
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    focus_prevent_scroll(&el);
                }
            } else if let Some(el) = panel_ref
                .get_untracked()
                .and_then(|p| {
                    p.query_selector(&format!("[data-agent-panel={}]", cur.as_str()))
                        .ok()
                        .flatten()
                })
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            {
                focus_prevent_scroll(&el);
            }
        }
        return_to_tabs.with_value(|r| *r.borrow_mut() = false);
        previous_view.with_value(|p| *p.borrow_mut() = cur);
    });

    let update_preferences = Callback::new(move |patch: serde_json::Map<String, Value>| {
        thread_error.set(String::new());
        let rev = prefs_shared
            .value
            .with_untracked(|p| p.as_ref().map(|p| p.revision).unwrap_or(0));
        let sess = session_sv.get_value();
        spawn_local(async move {
            match shared::save_preferences(Value::Object(patch), rev, &sess).await {
                Ok(updated) => {
                    prefs_shared.value.try_set(Some(updated));
                }
                Err(e) => {
                    thread_error.try_set(failure(&e));
                }
            }
        });
    });

    let snoozed = Memo::new(move |_| prefs_shared.value.with(pref_snoozed));
    let pinned = Memo::new(move |_| {
        prefs_shared
            .value
            .with(|p| pref_string_vec(p, "agentPinnedThreads"))
    });
    let archived = Memo::new(move |_| {
        prefs_shared
            .value
            .with(|p| pref_string_vec(p, "agentArchivedThreads"))
    });
    let send_key = Signal::derive(move || prefs_shared.value.with(pref_send_key));
    let grouping = Signal::derive(move || prefs_shared.value.with(pref_grouping));
    let notify_enabled = Signal::derive(move || prefs_shared.value.with(pref_notify));
    let follow_up_mode = Signal::derive(move || prefs_shared.value.with(pref_follow_up_mode));
    let auto_settle = Signal::derive(move || prefs_shared.value.with(pref_auto_settle));
    let diff_view = Signal::derive(move || prefs_shared.value.with(pref_diff_view));
    let starred_models = Signal::derive(move || {
        prefs_shared
            .value
            .with(|p| pref_string_vec(p, "agentStarredModels"))
    });
    let prompt_stash = Signal::derive(move || prefs_shared.value.with(pref_prompt_stash));
    let project_scripts_map =
        Signal::derive(move || prefs_shared.value.with(pref_project_scripts_map));
    let schedules = Signal::derive(move || prefs_shared.value.with(pref_schedules));
    let settled_prefs = Memo::new(move |_| prefs_shared.value.with(pref_settled));

    let toggle_star_model = Callback::new(move |model_id: String| {
        let mut next = starred_models.get_untracked();
        if next.contains(&model_id) {
            next.retain(|id| id != &model_id);
        } else {
            next.insert(0, model_id);
            next.truncate(32);
        }
        let mut map = serde_json::Map::new();
        map.insert("agentStarredModels".into(), json!(next));
        update_preferences.run(map);
    });

    let update_stash = Callback::new(move |entries: Vec<PromptStashEntry>| {
        let mut map = serde_json::Map::new();
        map.insert("agentPromptStash".into(), json!(entries));
        update_preferences.run(map);
    });

    let update_schedules = Callback::new(move |entries: Vec<AgentScheduleEntry>| {
        let mut map = serde_json::Map::new();
        map.insert("agentSchedules".into(), json!(entries));
        update_preferences.run(map);
    });

    let attach_chip = Callback::new(move |chip: ContextChip| {
        context_chips.update(|list| {
            if list.len() < 8 {
                list.push(chip);
            }
        });
    });

    let start_scratch = Callback::new(move |()| {
        if let Some(existing) = projects.with_untracked(|list| {
            list.as_ref()
                .and_then(|v| v.iter().find(|p| p.source_type == "scratch").cloned())
        }) {
            project_id.set(existing.id);
            thread_id.set(String::new());
            active_page.set(AgentPage::Workspace);
            view.set(View::Conversation);
            return;
        }
        let token = csrf_sv.get_value();
        let signal = lifetime.with_value(|c| c.signal());
        let body = json!({
            "id": random_uuid(),
            "name": "Scratch",
            "sourceType": "scratch",
            "sourceRef": "scratch",
        })
        .to_string();
        spawn_local(async move {
            match api::request::<AgentProject>(
                &format!("{BASE}/projects"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&token),
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(row) if !signal.aborted() => {
                    let pid = row.id.clone();
                    projects.try_update(|cur| {
                        let mut next = cur.take().unwrap_or_default();
                        next.push(row);
                        *cur = Some(next);
                    });
                    project_id.try_set(pid);
                    thread_id.try_set(String::new());
                    active_page.try_set(AgentPage::Workspace);
                    view.try_set(View::Conversation);
                }
                Err(e) if !signal.aborted() => {
                    create_error.try_set(failure(&e));
                }
                _ => {}
            }
        });
    });

    let snoozed_until = move |row_id: &str| -> Option<f64> {
        snoozed.with(|list| list.iter().find(|s| s.id == row_id).map(|s| s.until))
    };

    let is_snoozed =
        move |row_id: &str| -> bool { snoozed_until(row_id).unwrap_or(0.0) > now.get() };

    let is_woke =
        move |row_id: &str| -> bool { snoozed_until(row_id).is_some_and(|u| u <= now.get()) };

    let snooze_thread = Callback::new(move |(row_id, until): (String, Option<f64>)| {
        if until.is_some() && row_id == thread_id.get_untracked() {
            thread_id.set(String::new());
        }
        let cutoff = js_sys::Date::now() - 7.0 * 86400000.0;
        let mut list: Vec<SnoozedEntry> = snoozed
            .get_untracked()
            .into_iter()
            .filter(|item| item.id != row_id && item.until > cutoff)
            .collect();
        if let Some(u) = until {
            list.push(SnoozedEntry {
                id: row_id,
                until: u,
            });
        }
        if list.len() > 128 {
            list.drain(0..(list.len() - 128));
        }
        let mut map = serde_json::Map::new();
        map.insert(
            "agentSnoozed".into(),
            serde_json::to_value(list).unwrap_or(Value::Array(Vec::new())),
        );
        update_preferences.run(map);
    });

    // Wake notifications.
    let wake_notified: StoredValue<RefCell<HashSet<String>>, LocalStorage> =
        StoredValue::new_local(RefCell::new(HashSet::new()));
    Effect::new(move |_| {
        let cur_now = now.get();
        let sn = snoozed.get();
        let notify = notify_enabled.get();
        let Some(list) = threads.get() else { return };
        for row in list {
            let Some(until) = sn.iter().find(|s| s.id == row.id).map(|s| s.until) else {
                continue;
            };
            if until > cur_now {
                continue;
            }
            let key = format!("{}{}", row.id, until);
            let first = wake_notified.with_value(|s| s.borrow_mut().insert(key));
            if first && notify && (js_sys::Date::now() - until) < 120000.0 {
                show_notification(
                    "Snoozed thread is back",
                    &row.title,
                    &format!("wake-{}", row.id),
                    None,
                );
            }
        }
    });

    // Opening a woke thread clears its snooze.
    Effect::new(move |_| {
        let cur_now = now.get();
        let Some(t) = thread.get() else { return };
        if snoozed.with_untracked(|list| {
            list.iter()
                .find(|s| s.id == t.id)
                .is_some_and(|s| s.until <= cur_now)
        }) {
            snooze_thread.run((t.id, None));
        }
    });

    // Fetch provider usage limits when settings, details, or Usage page are open.
    Effect::new(move |_| {
        let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
        let open = settings.get() || details.get() || active_page.get() == AgentPage::Usage;
        let _ep = history_epoch.get();
        if !connected || !open {
            return;
        }
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        spawn_local(async move {
            if let Ok(val) = api::request::<AgentLimits>(
                &format!("{BASE}/provider/limits"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
                && !signal.aborted()
            {
                limits.try_set(Some(val));
            }
        });
    });

    let export_thread = Callback::new(move |(row, mode): (AgentThread, &'static str)| {
        let signal = lifetime.with_value(|c| c.signal());
        let project_name = projects
            .with_untracked(|p| {
                p.as_ref().and_then(|list| {
                    list.iter()
                        .find(|item| item.id == row.project_id)
                        .map(|i| i.name.clone())
                })
            })
            .unwrap_or_else(|| "Project".into());
        spawn_local(async move {
            let mut items: Vec<AgentMessage> = Vec::new();
            let mut before: Option<u64> = None;
            loop {
                let url = match before {
                    Some(b) => format!("{BASE}/threads/{}/messages?before={b}", row.id),
                    None => format!("{BASE}/threads/{}/messages", row.id),
                };
                match api::request::<AgentHistory>(
                    &url,
                    Req {
                        signal: Some(signal.clone()),
                        ..Default::default()
                    },
                )
                .await
                {
                    Ok(page) => {
                        let mut combined = page.items;
                        combined.extend(items);
                        items = combined;
                        before = page.next_before;
                        if before.is_none() {
                            break;
                        }
                    }
                    Err(e) => {
                        thread_error.try_set(failure(&e));
                        return;
                    }
                }
            }
            let text = markdown_thread(&row, &project_name, &items);
            if mode == "copy" {
                if write_clipboard(&text).await.is_err() {
                    thread_error.try_set("The request could not be completed.".into());
                }
            } else {
                let parts = js_sys::Array::of1(&JsValue::from_str(&text));
                let bag = web_sys::BlobPropertyBag::new();
                bag.set_type("text/markdown");
                if let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &bag)
                    && let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob)
                    && let Ok(link_el) = document().create_element("a")
                    && let Ok(link) = link_el.dyn_into::<web_sys::HtmlAnchorElement>()
                {
                    link.set_href(&url);
                    link.set_download(&format!("{}.md", sanitize_filename(&row.title)));
                    link.click();
                    let url_revoke = url.clone();
                    spawn_local(async move {
                        crate::sleep(1000).await;
                        let _ = web_sys::Url::revoke_object_url(&url_revoke);
                    });
                }
            }
        });
    });

    // Poll shared threads every 30s.
    Effect::new(move |_| {
        let _r = reload.get();
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        spawn_local(async move {
            while !signal.aborted() {
                if let Ok(val) = api::request::<SharedList>(
                    &format!("{BASE}/shared"),
                    Req {
                        signal: Some(signal.clone()),
                        ..Default::default()
                    },
                )
                .await
                    && !signal.aborted()
                {
                    let cur = shared_id.get_untracked();
                    if !cur.is_empty() && !val.items.iter().any(|i| i.thread.id == cur) {
                        shared_id.try_set(String::new());
                    }
                    shared.try_set(val.items);
                }
                crate::sleep(30000).await;
            }
        });
    });

    let shared_item = Memo::new(move |_| {
        let sid = shared_id.get();
        if sid.is_empty() {
            return None;
        }
        shared.with(|list| list.iter().find(|i| i.thread.id == sid).cloned())
    });

    #[derive(Clone, PartialEq)]
    enum ChatSlot {
        Shared(String),
        Thread(String),
        Draft(String),
        Empty(bool),
    }

    let has_thread = Memo::new(move |_| thread.with(Option::is_some));
    let has_projects = Memo::new(move |_| projects.with(Option::is_some));
    let review_run_id = Memo::new(move |_| {
        run.with(|r| {
            r.as_ref()
                .map(|r| r.id.clone())
                .unwrap_or_else(|| "empty".into())
        })
    });
    let chat_slot = Memo::new(move |_| {
        if let Some(t) = thread.get() {
            return ChatSlot::Thread(t.id);
        }
        if let Some(s) = shared_item.get() {
            return ChatSlot::Shared(s.thread.id);
        }
        if let Some(p) = project.get() {
            return ChatSlot::Draft(p.id);
        }
        ChatSlot::Empty(projects.with(Option::is_none))
    });

    Effect::new(move |_| {
        if !thread_id.with(String::is_empty) {
            shared_id.set(String::new());
        }
    });

    let toggle_pin = Callback::new(move |row_id: String| {
        let cur = pinned.get_untracked();
        let mut next: Vec<String> = if cur.contains(&row_id) {
            cur.into_iter().filter(|id| id != &row_id).collect()
        } else {
            let mut v = vec![row_id];
            v.extend(cur);
            v.truncate(128);
            v
        };
        next.truncate(128);
        let mut map = serde_json::Map::new();
        map.insert(
            "agentPinnedThreads".into(),
            serde_json::to_value(next).unwrap_or(Value::Array(Vec::new())),
        );
        update_preferences.run(map);
    });

    let toggle_archive = Callback::new(move |row_id: String| {
        let cur_arch = archived.get_untracked();
        let restoring = cur_arch.contains(&row_id);
        if !restoring && row_id == thread_id.get_untracked() {
            thread_id.set(String::new());
        }
        let next_arch: Vec<String> = if restoring {
            cur_arch.into_iter().filter(|id| id != &row_id).collect()
        } else {
            let mut v = vec![row_id.clone()];
            v.extend(cur_arch);
            v.truncate(128);
            v
        };
        let next_pin: Vec<String> = pinned
            .get_untracked()
            .into_iter()
            .filter(|id| id != &row_id)
            .collect();
        let mut map = serde_json::Map::new();
        map.insert(
            "agentArchivedThreads".into(),
            serde_json::to_value(next_arch).unwrap_or(Value::Array(Vec::new())),
        );
        map.insert(
            "agentPinnedThreads".into(),
            serde_json::to_value(next_pin).unwrap_or(Value::Array(Vec::new())),
        );
        update_preferences.run(map);
    });

    // Per-browser read markers.
    let seen_key = StoredValue::new(format!("dar.agent.seen.{}", session.user.id));
    let seen: RwSignal<Option<HashMap<String, u64>>> = RwSignal::new(
        storage_get(local_storage(), &seen_key.get_value())
            .and_then(|raw| serde_json::from_str(&raw).ok()),
    );

    Effect::new(move |_| {
        let Some(list) = threads.get() else { return };
        let cur_thread = thread.get();
        seen.update(|cur| {
            let mut next = cur
                .clone()
                .unwrap_or_else(|| list.iter().map(|r| (r.id.clone(), r.revision)).collect());
            if let Some(ref t) = cur_thread {
                next.insert(t.id.clone(), t.revision);
            }
            let valid_ids: HashSet<&str> = list.iter().map(|r| r.id.as_str()).collect();
            next.retain(|id, _| valid_ids.contains(id.as_str()));
            if let Ok(raw) = serde_json::to_string(&next) {
                let _ = storage_set(local_storage(), &seen_key.get_value(), &raw);
            }
            *cur = Some(next);
        });
    });

    let is_working = move |row: &AgentThread| -> bool {
        let cur_tid = thread_id.get();
        if row.id == cur_tid
            && let Some(r) = run.get()
        {
            return active_agent_run(&r.status);
        }
        active_agent_run(row.run_status.as_deref().unwrap_or(""))
    };

    let is_working_untracked = move |row: &AgentThread| -> bool {
        let cur_tid = thread_id.get_untracked();
        if row.id == cur_tid
            && let Some(r) = run.get_untracked()
        {
            return active_agent_run(&r.status);
        }
        active_agent_run(row.run_status.as_deref().unwrap_or(""))
    };

    let unread = move |row: &AgentThread| -> bool {
        let cur_tid = thread_id.get();
        row.id != cur_tid
            && !is_working(row)
            && seen.with(|s| {
                s.as_ref()
                    .is_some_and(|map| row.revision > *map.get(&row.id).unwrap_or(&0))
            })
    };

    // Desktop notifications when a background run finishes.
    Effect::new(move |_| {
        let notify = notify_enabled.get();
        let Some(list) = threads.get() else { return };
        let cur_selected = thread_id.get_untracked();
        for row in list {
            let after = row.run_status.clone().unwrap_or_default();
            let before =
                statuses.with_value(|m| m.borrow_mut().insert(row.id.clone(), after.clone()));
            if let Some(prev) = before
                && active_agent_run(&prev)
                && !active_agent_run(&after)
                && notify
                && (document().hidden() || row.id != cur_selected)
            {
                let title = if after == "completed" {
                    "Agent finished".into()
                } else {
                    format!("Agent {after}")
                };
                let rid = row.id.clone();
                let pid = row.project_id.clone();
                show_notification(
                    &title,
                    &row.title,
                    &row.id,
                    Some(Box::new(move || {
                        let _ = window().focus();
                        thread_id.try_set(rid.clone());
                        project_id.try_set(pid.clone());
                    })),
                );
            }
        }
    });

    let changed = Callback::new(move |row: AgentThread| {
        threads.update(|cur| {
            if let Some(list) = cur {
                for item in list.iter_mut() {
                    if item.id == row.id {
                        *item = row.clone();
                    }
                }
            }
        });
    });

    let is_settled = move |row: &AgentThread| -> bool {
        !is_working(row)
            && settled_prefs.with(|list| {
                list.iter()
                    .any(|s| s.id == row.id && s.revision == row.revision)
            })
    };

    let settle = Callback::new(move |row: AgentThread| {
        if !prefs_loaded.get_untracked() || settling.get_untracked() || is_working_untracked(&row) {
            return;
        }
        settling.set(true);
        thread_error.set(String::new());
        let cur_settled = settled_prefs.get_untracked();
        let already = cur_settled
            .iter()
            .any(|s| s.id == row.id && s.revision == row.revision);
        let mut entries: Vec<SettledEntry> =
            cur_settled.into_iter().filter(|s| s.id != row.id).collect();
        if !already {
            entries.push(SettledEntry {
                id: row.id,
                revision: row.revision,
            });
        }
        let rev = prefs_shared
            .value
            .with_untracked(|p| p.as_ref().map(|p| p.revision).unwrap_or(0));
        let sess = session_sv.get_value();
        spawn_local(async move {
            let mut map = serde_json::Map::new();
            map.insert(
                "agentSettledThreads".into(),
                serde_json::to_value(entries).unwrap_or(Value::Array(Vec::new())),
            );
            match shared::save_preferences(Value::Object(map), rev, &sess).await {
                Ok(updated) => {
                    prefs_shared.value.try_set(Some(updated));
                }
                Err(e) => {
                    thread_error.try_set(failure(&e));
                }
            }
            settling.try_set(false);
        });
    });

    let start_rename = Callback::new(move |row: AgentThread| {
        renaming.set(row.id);
        rename_value.set(row.title);
    });

    let commit_rename = Callback::new(move |row: AgentThread| {
        let title = rename_value.get_untracked().trim().to_owned();
        renaming.set(String::new());
        if title.is_empty() || title == row.title {
            return;
        }
        let mut optimistic = row.clone();
        optimistic.title = title.clone();
        changed.run(optimistic);
        let signal = lifetime.with_value(|c| c.signal());
        let token = csrf_sv.get_value();
        let body = json!({ "title": title }).to_string();
        spawn_local(async move {
            match api::request::<AgentThread>(
                &format!("{BASE}/threads/{}", row.id),
                Req {
                    method: "PATCH",
                    body: Some(body),
                    csrf: Some(&token),
                    signal: Some(signal),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(val) => changed.run(val),
                Err(e) => {
                    changed.run(row);
                    thread_error.try_set(failure(&e));
                }
            }
        });
    });

    let remove_thread = Callback::new(move |row: AgentThread| {
        delete_busy.set(true);
        delete_error.set(String::new());
        let signal = lifetime.with_value(|c| c.signal());
        let token = csrf_sv.get_value();
        let uid = user_id_sv.get_value();
        spawn_local(async move {
            match api::request::<Value>(
                &format!("{BASE}/threads/{}", row.id),
                Req {
                    method: "DELETE",
                    csrf: Some(&token),
                    signal: Some(signal),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    threads.try_update(|cur| {
                        if let Some(list) = cur {
                            list.retain(|i| i.id != row.id);
                        }
                    });
                    if row.id == thread_id.get_untracked() {
                        thread_id.try_set(String::new());
                    }
                    deleting.try_set(None);
                    storage_remove(
                        session_storage(),
                        &format!("dar.agent.draft.{uid}.{}", row.id),
                    );
                    let p = pinned.get_untracked();
                    let a = archived.get_untracked();
                    if p.contains(&row.id) || a.contains(&row.id) {
                        let mut map = serde_json::Map::new();
                        map.insert(
                            "agentPinnedThreads".into(),
                            serde_json::to_value(
                                p.into_iter().filter(|id| id != &row.id).collect::<Vec<_>>(),
                            )
                            .unwrap_or(Value::Array(Vec::new())),
                        );
                        map.insert(
                            "agentArchivedThreads".into(),
                            serde_json::to_value(
                                a.into_iter().filter(|id| id != &row.id).collect::<Vec<_>>(),
                            )
                            .unwrap_or(Value::Array(Vec::new())),
                        );
                        update_preferences.run(map);
                    }
                }
                Err(e) => {
                    delete_error.try_set(failure(&e));
                }
            }
            delete_busy.try_set(false);
        });
    });

    let open_review = Callback::new(move |()| {
        active_surface.set(SurfaceKind::Diff);
        review.set(true);
        view.set(View::Changes);
    });

    let open_surface = Callback::new(move |kind: SurfaceKind| {
        active_surface.set(kind);
        review.set(true);
        view.set(View::Changes);
    });

    let open_file_in_surface = Callback::new(move |path: String| {
        surface_file.set(Some(path));
        active_surface.set(SurfaceKind::Files);
        review.set(true);
        view.set(View::Changes);
    });

    let close_review = Callback::new(move |()| {
        review.set(false);
        maximized.set(false);
        view.set(View::Conversation);
        request_animation_frame(move || {
            if let Some(btn) = panel_ref
                .get_untracked()
                .and_then(|p| {
                    p.query_selector("[aria-label=\"Show changes\"]")
                        .ok()
                        .flatten()
                })
                .and_then(|e| e.dyn_into::<web_sys::HtmlButtonElement>().ok())
            {
                let _ = btn.focus();
            }
        });
    });

    let stop = Callback::new(move |()| {
        let rid = run_id.get_untracked();
        if rid.is_empty() {
            return;
        }
        let tid = thread_id.get_untracked();
        if let Some(cur_run) = run.get_untracked()
            && (!active_agent_run(&cur_run.status) || cur_run.thread_id != tid)
        {
            return;
        }
        let signal = lifetime.with_value(|c| c.signal());
        let token = csrf_sv.get_value();
        spawn_local(async move {
            match api::request::<AgentRun>(
                &format!("{BASE}/runs/{rid}/cancel"),
                Req {
                    method: "POST",
                    csrf: Some(&token),
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(val) => {
                    if !signal.aborted() {
                        stored_run.try_set(Some(val));
                    }
                }
                Err(e) => {
                    if !signal.aborted() {
                        run_error.try_set(failure(&e));
                    }
                }
            }
        });
    });

    let copy_thread = Callback::new(move |item: AgentThread| {
        spawn_local(async move {
            let copied = if let Ok(href) = window().location().href()
                && let Ok(url) = web_sys::Url::new(&href)
            {
                let sp = url.search_params();
                sp.set("thread", &item.id);
                sp.set("project", &item.project_id);
                write_clipboard(&url.href()).await.is_ok()
            } else {
                false
            };
            if !copied {
                thread_error.try_set(
                    "Could not copy the thread link. Open the thread and copy its address.".into(),
                );
            }
        });
    });

    let started = Callback::new(
        move |(row, execute, files): (AgentThread, bool, Vec<SendWrapper<web_sys::File>>)| {
            auto_start.with_value(|m| {
                m.borrow_mut().insert(row.id.clone(), execute);
            });
            if !files.is_empty() {
                start_files.with_value(|m| {
                    m.borrow_mut().insert(row.id.clone(), files);
                });
            }
            let rid = row.id.clone();
            let pid = row.project_id.clone();
            threads.update(|cur| {
                let mut next = vec![row];
                if let Some(existing) = cur.take() {
                    next.extend(existing.into_iter().filter(|item| item.id != rid));
                }
                *cur = Some(next);
            });
            project_id.set(pid);
            thread_id.set(rid);
            view.set(View::Conversation);
        },
    );

    let slash_commands = Callback::new(move |controls: SlashControls| -> Vec<SlashCommand> {
        let mut list = vec![
            SlashCommand {
                name: "new".into(),
                hint: "Start a new thread".into(),
                argument: None,
                run: Callback::new(move |_| choose_new_thread.run(())),
            },
            SlashCommand {
                name: "model".into(),
                hint: "Choose the model".into(),
                argument: None,
                run: Callback::new(move |_| {
                    request_animation_frame(|| {
                        if let Some(btn) = document()
                            .query_selector(".agent-model-trigger")
                            .ok()
                            .flatten()
                            .and_then(|e| e.dyn_into::<web_sys::HtmlButtonElement>().ok())
                        {
                            btn.click();
                        }
                    });
                }),
            },
        ];
        if !controls.efforts.is_empty() {
            list.push(SlashCommand {
                name: "effort".into(),
                hint: format!("Reasoning: {}", controls.efforts.join(", ")),
                argument: Some(controls.efforts),
                run: controls.set_effort,
            });
        }
        list.extend([
            SlashCommand {
                name: "changes".into(),
                hint: "Open the changes panel".into(),
                argument: None,
                run: Callback::new(move |_| open_review.run(())),
            },
            SlashCommand {
                name: "files".into(),
                hint: "Browse workspace files".into(),
                argument: None,
                run: Callback::new(move |_| open_surface.run(SurfaceKind::Files)),
            },
            SlashCommand {
                name: "preview".into(),
                hint: "Preview HTML, SVG, or Markdown workspace files".into(),
                argument: None,
                run: Callback::new(move |_| open_surface.run(SurfaceKind::Preview)),
            },
            SlashCommand {
                name: "prs".into(),
                hint: "Open Dar Git pull requests & issues".into(),
                argument: None,
                run: Callback::new(move |_| open_surface.run(SurfaceKind::PullRequests)),
            },
            SlashCommand {
                name: "usage".into(),
                hint: "Open usage & limits page".into(),
                argument: None,
                run: Callback::new(move |_| active_page.set(AgentPage::Usage)),
            },
            SlashCommand {
                name: "automations".into(),
                hint: "Open automations & scheduled prompts".into(),
                argument: None,
                run: Callback::new(move |_| active_page.set(AgentPage::Automations)),
            },
            SlashCommand {
                name: "terminal".into(),
                hint: "Show command output".into(),
                argument: None,
                run: Callback::new(move |_| {
                    drawer_tab.set("output");
                    terminal.set(true);
                }),
            },
            SlashCommand {
                name: "shell".into(),
                hint: "Open an interactive terminal in the working copy".into(),
                argument: None,
                run: Callback::new(move |_| {
                    drawer_tab.set("shell");
                    terminal.set(true);
                }),
            },
            SlashCommand {
                name: "find".into(),
                hint: "Find in this thread".into(),
                argument: None,
                run: Callback::new(move |_| find_open.set(true)),
            },
            SlashCommand {
                name: "rename".into(),
                hint: "Rename this thread".into(),
                argument: None,
                run: Callback::new(move |_| {
                    if let Some(t) = thread.get_untracked() {
                        start_rename.run(t);
                    }
                }),
            },
            SlashCommand {
                name: "settle".into(),
                hint: "Settle or restore this thread".into(),
                argument: None,
                run: Callback::new(move |_| {
                    if let Some(t) = thread.get_untracked() {
                        settle.run(t);
                    }
                }),
            },
            SlashCommand {
                name: "settings".into(),
                hint: "Open Agent settings".into(),
                argument: None,
                run: Callback::new(move |_| settings.set(true)),
            },
        ]);
        list
    });

    // Helper to render a list of thread rows.
    let active_run_created = Memo::new(move |_| run.with(|r| r.as_ref().map(|r| r.created_at)));
    let render_rows = move |items: Vec<AgentThread>| {
        let proj_list = projects.get().unwrap_or_default();
        let cur_tid = thread_id.get();
        let cur_renaming = renaming.get();
        let cur_grouping = grouping.get();
        let cur_pinned = pinned.get();
        let cur_now = now.get();
        let cur_run_created = active_run_created.get();
        let cur_settling = settling.get();
        let loaded = prefs_loaded.get();

        view! {
            <div class="agent-thread-list">
                {items.into_iter().map(|item| {
                    let pname = proj_list.iter().find(|p| p.id == item.project_id).map(|p| p.name.clone()).unwrap_or_else(|| "Project".into());
                    let working = is_working(&item);
                    let settled_row = is_settled(&item);
                    let is_pinned = cur_pinned.contains(&item.id);
                    let fresh = unread(&item);
                    let sn_until = snoozed_until(&item.id);
                    let woke_row = is_woke(&item.id);
                    let snoozed_row = is_snoozed(&item.id);

                    if cur_renaming == item.id {
                        let item_commit = item.clone();
                        return view! {
                            <div class="agent-thread-row selected">
                                <ProjectMark name=pname/>
                                <input class="agent-rename" autofocus aria-label="Thread title" prop:value=move || rename_value.get() maxlength="200"
                                    on:input=move |e| rename_value.set(event_target_value(&e))
                                    on:blur=move |_| commit_rename.run(item_commit.clone())
                                    on:keydown=move |e| {
                                        if e.key() == "Enter" {
                                            e.prevent_default();
                                            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                                                let _ = el.blur();
                                            }
                                        } else if e.key() == "Escape" {
                                            e.prevent_default();
                                            e.stop_propagation();
                                            renaming.set(String::new());
                                        }
                                    }/>
                            </div>
                        }.into_any();
                    }

                    let item_sv = StoredValue::new(item.clone());
                    let selected_row = item.id == cur_tid;
                    let mut row_class = String::from("agent-thread-row");
                    if selected_row { row_class.push_str(" selected"); }
                    if settled_row { row_class.push_str(" settled"); }
                    if fresh { row_class.push_str(" unread"); }

                    let actions_cb = Callback::new(move |()| {
                        let row = item_sv.get_value();
                        let loaded_now = prefs_loaded.get_untracked();
                        let working_now = is_working_untracked(&row);
                        let settled_now = !working_now && settled_prefs.with_untracked(|l| l.iter().any(|s| s.id == row.id && s.revision == row.revision));
                        let pinned_now = pinned.with_untracked(|p| p.contains(&row.id));
                        let archived_now = archived.with_untracked(|a| a.contains(&row.id));
                        let snoozed_now = snoozed.with_untracked(|s| s.iter().any(|x| x.id == row.id));
                        let id_pin = row.id.clone();
                        let row_ren = row.clone();
                        let row_set = row.clone();
                        let row_copy = row.clone();
                        let id_unsnooze = row.id.clone();
                        let row_custom = row.clone();
                        let row_exp = row.clone();
                        let id_arch = row.id.clone();
                        let row_del = row.clone();
                        let snooze_action = if snoozed_now {
                            MenuAction::new("Unsnooze thread", "clock").run(move || snooze_thread.run((id_unsnooze.clone(), None)))
                        } else {
                            let mut sub: Vec<MenuAction> = snooze_presets()
                                .into_iter()
                                .map(|(lbl, until)| {
                                    let rid = row.id.clone();
                                    MenuAction::plain(lbl).run(move || snooze_thread.run((rid.clone(), Some(until))))
                                })
                                .collect();
                            sub.push(MenuAction::plain("Custom\u{2026}").run(move || {
                                snooze_at.set(String::new());
                                snoozing.set(Some(row_custom.clone()));
                            }));
                            MenuAction::new("Snooze", "clock").disabled(!loaded_now || working_now).items(sub)
                        };
                        vec![
                            MenuAction::new(if pinned_now { "Unpin thread" } else { "Pin thread" }, "pin").disabled(!loaded_now).run(move || toggle_pin.run(id_pin.clone())),
                            MenuAction::new("Rename thread", "pencil").run(move || start_rename.run(row_ren.clone())),
                            MenuAction::new(if settled_now { "Restore thread" } else { "Settle thread" }, "check").disabled(settling.get_untracked() || !loaded_now || working_now).run(move || settle.run(row_set.clone())),
                            MenuAction::new("Copy thread link", "share").run(move || copy_thread.run(row_copy.clone())),
                            snooze_action,
                            MenuAction::new("Export as Markdown", "download").run(move || export_thread.run((row_exp.clone(), "download"))),
                            MenuAction::new(if archived_now { "Unarchive thread" } else { "Archive thread" }, "archive").disabled(!loaded_now || working_now).run(move || toggle_archive.run(id_arch.clone())),
                            MenuAction::new("Delete thread", "trash").disabled(working_now).run(move || { delete_error.set(String::new()); deleting.set(Some(row_del.clone())); }),
                        ]
                    });

                    let id_sel = item.id.clone();
                    let pid_sel = item.project_id.clone();
                    let row_dbl = item.clone();
                    let row_f2 = item.clone();
                    let id_woke = item.id.clone();
                    let row_settle_btn = item.clone();
                    let working_label = if selected_row && let Some(created_at) = cur_run_created {
                        duration(js_sys::Date::now() - created_at)
                    } else {
                        "Working".into()
                    };

                    view! {
                        <div class=row_class on:contextmenu=move |e| {
                            if let Some(el) = e.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                                && let Ok(Some(trig)) = el.query_selector(".agent-thread-actions .ui-menu-trigger")
                                && let Ok(btn) = trig.dyn_into::<web_sys::HtmlButtonElement>()
                            {
                                e.prevent_default();
                                btn.click();
                            }
                        }>
                            <button class="agent-thread-select"
                                aria-current=selected_row.then_some("page")
                                title=format!("{pname} / {}", item.title)
                                on:click=move |_| {
                                    thread_id.set(id_sel.clone());
                                    project_id.set(pid_sel.clone());
                                    find_open.set(false);
                                    active_page.set(AgentPage::Workspace);
                                    view.set(View::Conversation);
                                }
                                on:dblclick=move |_| start_rename.run(row_dbl.clone())
                                on:keydown=move |e| if e.key() == "F2" { e.prevent_default(); start_rename.run(row_f2.clone()); }>
                                {(!(cur_grouping == "project" && !is_pinned)).then(|| view! { <ProjectMark name=pname.clone()/> })}
                                <span class="sr-only">{format!("{pname}: ")}</span>
                                <span class="agent-thread-title">{item.title.clone()}</span>
                                {is_pinned.then(|| view! { <Icon name="pin" class="agent-pin-mark"/> })}
                                {fresh.then(|| view! { <span class="agent-unread" role="img" aria-label="Unread"/> })}
                            </button>
                            <span class="agent-thread-meta">
                                {if woke_row {
                                    view! {
                                        <button class="agent-woke" aria-label=format!("Dismiss Woke for {}", item.title) on:click=move |_| snooze_thread.run((id_woke.clone(), None))>
                                            <Icon name="clock"/>"Woke"
                                        </button>
                                    }.into_any()
                                } else if snoozed_row && let Some(until) = sn_until {
                                    view! {
                                        <span class="agent-snooze-time" title=format!("Wakes {}", locale_string(until))>
                                            <Icon name="clock"/>{wake_label(until)}
                                        </span>
                                    }.into_any()
                                } else if working {
                                    view! {
                                        <span class="agent-thread-status"><span class="agent-pulse" aria-hidden="true"/>{working_label}</span>
                                    }.into_any()
                                } else {
                                    view! {
                                        <time datetime=iso_string(item.modified_at) title=locale_string(item.modified_at)>{ago(item.modified_at, cur_now)}</time>
                                    }.into_any()
                                }}
                            </span>
                            <span class="agent-thread-actions">
                                {(!working).then(|| view! {
                                    <button class="icon-button"
                                        aria-label=if settled_row { format!("Restore {}", item.title) } else { format!("Settle {}", item.title) }
                                        title=if settled_row { "Restore thread" } else { "Settle thread" }
                                        disabled=cur_settling || !loaded
                                        on:click=move |_| settle.run(row_settle_btn.clone())>
                                        <Icon name=if settled_row { "undo" } else { "check" }/>
                                    </button>
                                })}
                                <ActionMenu label=format!("Actions for {}", item.title) actions=actions_cb><Icon name="more"/></ActionMenu>
                            </span>
                        </div>
                    }.into_any()
                }).collect_view()}
            </div>
        }
    };

    let render_grouped = move |items: Vec<AgentThread>| {
        if grouping.get() != "project" {
            return render_rows(items).into_any();
        }
        let proj_list = projects.get().unwrap_or_default();
        let q_empty = query.with(String::is_empty);
        let collapsed = collapsed_groups.get();
        proj_list
            .into_iter()
            .filter(|row| items.iter().any(|i| i.project_id == row.id) || q_empty)
            .map(|row| {
                let own: Vec<AgentThread> = items.iter().filter(|i| i.project_id == row.id).cloned().collect();
                let closed = collapsed.contains(&row.id);
                let rid_toggle = row.id.clone();
                let rid_new = row.id.clone();
                view! {
                    <div class="agent-project-group">
                        <div class="agent-project-heading">
                            <button aria-expanded=(!closed).to_string() on:click=move |_| {
                                collapsed_groups.update(|c| {
                                    if c.contains(&rid_toggle) { c.retain(|id| id != &rid_toggle); } else { c.push(rid_toggle.clone()); }
                                });
                            }>
                                <Icon name="chevron"/>
                                <ProjectMark name=row.name.clone()/>
                                <span>{row.name.clone()}</span>
                            </button>
                            <button class="icon-button" aria-label=format!("New thread in {}", row.name) title="New thread" on:click=move |_| {
                                thread_id.set(String::new());
                                project_id.set(rid_new.clone());
                                view.set(View::Conversation);
                                request_animation_frame(|| {
                                    if let Some(el) = document().get_element_by_id("agent-message").and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
                                        let _ = el.focus();
                                    }
                                });
                            }>
                                <Icon name="plus"/>
                            </button>
                        </div>
                        {(!closed).then(|| {
                            if own.is_empty() {
                                view! { <p class="agent-group-empty">"No threads"</p> }.into_any()
                            } else {
                                render_rows(own).into_any()
                            }
                        })}
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };

    let shell_class = move || {
        let mut cls = format!("app-shell agent-shell agent-view-{}", view.get().as_str());
        if !review.get() {
            cls.push_str(" agent-review-hidden");
        }
        if !nav.get() {
            cls.push_str(" agent-nav-hidden");
        }
        if !details.get() {
            cls.push_str(" agent-details-hidden");
        }
        if maximized.get() && review.get() {
            cls.push_str(" agent-review-maximized");
        }
        cls
    };

    view! {
        <div node_ref=shell_ref class=shell_class>
            <WorkspaceHeader app="agent" session=session.clone() on_signed_out=on_signed_out on_settings=Callback::new(move |_| settings.set(true))/>
            <nav node_ref=tabs_ref class="agent-mobile-views" aria-label="Agent views">
                <button class="icon-button agent-mobile-back" aria-label="Back to threads" on:click=move |_| view.set(View::Threads)><Icon name="back"/></button>
                {[
                    (View::Conversation, "Conversation"),
                    (View::Details, "Details"),
                    (View::Changes, "Changes"),
                    (View::Terminal, "Terminal"),
                ].into_iter().map(|(val, label)| view! {
                    <button aria-current=move || (view.get() == val).then_some("page") on:click=move |_| view.set(val)>{label}</button>
                }).collect_view()}
            </nav>
            <div class="agent-layout" node_ref=panel_ref on:keydown=move |e| {
                if e.key() == "Escape" && view.get_untracked() != View::Conversation && editor.with_untracked(Option::is_none) && !settings.get_untracked() {
                    e.prevent_default();
                    return_to_tabs.with_value(|r| *r.borrow_mut() = true);
                    view.set(View::Conversation);
                }
            }>
                <aside class="agent-sidebar" data-agent-panel="threads" tabindex="-1" aria-label="Threads">
                    <h1 class="sr-only">"Agent"</h1>
                    <button type="button" class="new-button ui-new agent-new-thread" disabled=move || projects.with(Option::is_none) on:click=move |_| choose_new_thread.run(())>
                        <Icon name="pencil"/>"New thread"
                    </button>
                    <div class="agent-search-row">
                        <label class="agent-thread-search">
                            <Icon name="search"/>
                            <span class="sr-only">"Search threads"</span>
                            <input node_ref=search_ref type="search" placeholder="Filter threads" prop:value=move || query.get()
                                on:input=move |e| query.set(event_target_value(&e))
                                on:keydown=move |e| if e.key() == "Escape" { e.prevent_default(); query.set(String::new()); }/>
                        </label>
                    </div>
                    <div class="agent-sidebar-filters" role="group" aria-label="Filter thread list">
                        {[("all", "All"), ("working", "Working"), ("unread", "Unread"), ("pinned", "Pinned")].into_iter().map(|(k, lbl)| view! {
                            <button type="button"
                                class=move || if sidebar_filter.get() == k { "agent-filter-pill is-active" } else { "agent-filter-pill" }
                                aria-pressed=move || (sidebar_filter.get() == k).to_string()
                                on:click=move |_| sidebar_filter.set(k)>
                                {lbl}
                            </button>
                        }).collect_view()}
                    </div>
                    {move || {
                        let e = error.get();
                        (!e.is_empty()).then(|| view! { <div role="alert"><p>{e}</p><button class="text-button" on:click=move |_| reload.update(|n| *n += 1)>"Retry projects"</button></div> })
                    }}
                    {move || {
                        let e = thread_error.get();
                        (!e.is_empty()).then(|| view! { <div role="alert"><p>{e}</p><button class="text-button" on:click=move |_| reload.update(|n| *n += 1)>"Retry threads"</button></div> })
                    }}
                    {move || {
                        let e = prefs_error.get();
                        (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> })
                    }}
                    <div class="agent-thread-shelves">
                        {move || {
                            let Some(all_threads) = threads.get() else {
                                return view! { <p role="status">"Loading threads\u{2026}"</p> }.into_any();
                            };
                            let proj_list = projects.get().unwrap_or_default();
                            let needle = query.get().to_lowercase();
                            let q_nonempty = !needle.is_empty();
                            let cur_arch = archived.get();
                            let cur_pin = pinned.get();

                            let sf = sidebar_filter.get();
                            let filtered: Vec<AgentThread> = all_threads
                                .iter()
                                .filter(|row| {
                                    let pname = proj_list.iter().find(|p| p.id == row.project_id).map(|p| p.name.as_str()).unwrap_or("");
                                    let text_ok = format!("{} {pname}", row.title).to_lowercase().contains(&needle);
                                    if !text_ok {
                                        return false;
                                    }
                                    match sf {
                                        "working" => is_working(row),
                                        "unread" => unread(row),
                                        "pinned" => cur_pin.contains(&row.id),
                                        _ => true,
                                    }
                                })
                                .cloned()
                                .collect();
                            let visible: Vec<AgentThread> = filtered.iter().filter(|r| !cur_arch.contains(&r.id) && (!is_snoozed(&r.id) || is_working(r))).cloned().collect();
                            let snoozed_rows: Vec<AgentThread> = filtered.iter().filter(|r| !cur_arch.contains(&r.id) && is_snoozed(&r.id) && !is_working(r)).cloned().collect();
                            let pinned_rows: Vec<AgentThread> = visible.iter().filter(|r| cur_pin.contains(&r.id) && !is_working(r)).cloned().collect();
                            let active: Vec<AgentThread> = visible.iter().filter(|r| !is_working(r) && !is_settled(r) && !cur_pin.contains(&r.id)).cloned().collect();
                            let working_rows: Vec<AgentThread> = visible.iter().filter(|r| is_working(r)).cloned().collect();
                            let settled_rows: Vec<AgentThread> = visible.iter().filter(|r| is_settled(r) && !cur_pin.contains(&r.id)).cloned().collect();
                            let shared_all = shared.get();
                            let cur_sid = shared_id.get();

                            view! {
                                {(!pinned_rows.is_empty()).then(|| view! {
                                    <div class="agent-pinned" aria-label="Pinned threads" role="group">{render_rows(pinned_rows)}</div>
                                })}
                                {render_grouped(active)}
                                {all_threads.is_empty().then(|| view! { <p class="muted small">"No threads yet."</p> })}
                                {(!all_threads.is_empty() && filtered.is_empty()).then(|| view! { <p role="status" class="muted small">"No matching threads."</p> })}
                                <div class="agent-shelves">
                                    {(!working_rows.is_empty()).then(|| {
                                        let len = working_rows.len();
                                        view! { <details open><summary>"Working "<span>{len}</span></summary>{render_rows(working_rows)}</details> }
                                    })}
                                    {(!shared_all.is_empty()).then(|| {
                                        let len = shared_all.len();
                                        let shared_filtered: Vec<SharedThread> = shared_all
                                            .into_iter()
                                            .filter(|item| format!("{} {}", item.thread.title, item.owner).to_lowercase().contains(&needle))
                                            .collect();
                                        view! {
                                            <details open>
                                                <summary>"Shared with me "<span>{len}</span></summary>
                                                <div class="agent-thread-list">
                                                    {shared_filtered.into_iter().map(|item| {
                                                        let is_sel = item.thread.id == cur_sid;
                                                        let sid = item.thread.id.clone();
                                                        let first_owner = item.owner.split(' ').next().unwrap_or("").to_owned();
                                                        let shared_title = format!("{} / {}", item.owner, item.thread.title);
                                                        let shared_by = format!("Shared by {}: ", item.owner);
                                                        view! {
                                                            <div class=if is_sel { "agent-thread-row selected" } else { "agent-thread-row" }>
                                                                <button class="agent-thread-select" aria-current=is_sel.then_some("page") title=shared_title
                                                                    on:click=move |_| { thread_id.set(String::new()); shared_id.set(sid.clone()); view.set(View::Conversation); }>
                                                                    <ProjectMark name=item.project.clone()/>
                                                                    <span class="sr-only">{shared_by}</span>
                                                                    <span class="agent-thread-title">{item.thread.title}</span>
                                                                </button>
                                                                <span class="agent-thread-meta">{first_owner}</span>
                                                            </div>
                                                        }
                                                    }).collect_view()}
                                                </div>
                                            </details>
                                        }
                                    })}
                                    {(!snoozed_rows.is_empty()).then(|| {
                                        let len = snoozed_rows.len();
                                        view! { <details open=q_nonempty><summary>"Snoozed "<span>{len}</span></summary>{render_rows(snoozed_rows)}</details> }
                                    })}
                                    <details open=q_nonempty><summary>"Settled "<span>{settled_rows.len()}</span></summary>{render_rows(settled_rows)}</details>
                                </div>
                            }.into_any()
                        }}
                    </div>
                    <nav class="agent-utility-dock" aria-label="Workspace pages">
                        <button type="button" class=move || if active_page.get() == AgentPage::Workspace { "agent-dock-btn is-active" } else { "agent-dock-btn" }
                            aria-current=move || (active_page.get() == AgentPage::Workspace).then_some("page")
                            on:click=move |_| { active_page.set(AgentPage::Workspace); view.set(View::Conversation); }>
                            <Icon name="chat"/><span>"Threads"</span>
                        </button>
                        <button type="button" class=move || if active_page.get() == AgentPage::Usage { "agent-dock-btn is-active" } else { "agent-dock-btn" }
                            aria-current=move || (active_page.get() == AgentPage::Usage).then_some("page")
                            on:click=move |_| { active_page.set(AgentPage::Usage); view.set(View::Conversation); }>
                            <Icon name="chart"/><span>"Usage"</span>
                        </button>
                        <button type="button" class=move || if active_page.get() == AgentPage::PullRequests { "agent-dock-btn is-active" } else { "agent-dock-btn" }
                            aria-current=move || (active_page.get() == AgentPage::PullRequests).then_some("page")
                            on:click=move |_| { active_page.set(AgentPage::PullRequests); view.set(View::Conversation); }>
                            <Icon name="git"/><span>"PRs"</span>
                        </button>
                        <button type="button" class=move || if active_page.get() == AgentPage::Automations { "agent-dock-btn is-active" } else { "agent-dock-btn" }
                            aria-current=move || (active_page.get() == AgentPage::Automations).then_some("page")
                            on:click=move |_| { active_page.set(AgentPage::Automations); view.set(View::Conversation); }>
                            <Icon name="clock"/><span>"Automations"</span>
                        </button>
                    </nav>
                    <AppSettingsButton app="agent" on_click=Callback::new(move |_| settings.set(true))/>
                    <Resizer label="Resize threads" side="right" value=sidebar_width min=208.0 max=420.0 on_change=Callback::new(move |v| sidebar_width.set(v))/>
                </aside>
                <main class="agent-center" data-agent-panel="conversation" tabindex="-1" aria-label="Agent conversation">
                    <header class="agent-thread-heading">
                        <button class="icon-button agent-desktop-toggle"
                            aria-label=move || if nav.get() { "Hide threads" } else { "Show threads" }
                            aria-expanded=move || nav.get().to_string()
                            on:click=move |_| nav.update(|v| *v = !*v)>
                            <Icon name="menu"/>
                        </button>
                        {move || project.get().map(|p| view! {
                            <span class="agent-heading-project"><ProjectMark name=p.name.clone()/>{p.name}<span>"/"</span></span>
                        })}
                        <h2>{move || {
                            if let Some(t) = thread.get() {
                                t.title
                            } else if let Some(s) = shared_item.get() {
                                s.thread.title
                            } else {
                                "New thread".into()
                            }
                        }}</h2>
                        {move || has_thread.get().then(|| {
                            view! {
                                <ActionMenu label="Thread actions" actions=Callback::new(move |_| {
                                    let Some(row) = thread.get_untracked() else { return Vec::new(); };
                                    let loaded_now = prefs_loaded.get_untracked();
                                    let working_now = is_working_untracked(&row);
                                    let settled_now = !working_now && settled_prefs.with_untracked(|l| l.iter().any(|s| s.id == row.id && s.revision == row.revision));
                                    let pinned_now = pinned.with_untracked(|p| p.contains(&row.id));
                                    let connected = provider.with_untracked(|p| p.as_ref().is_some_and(|p| p.connected));
                                    let row_ren = row.clone();
                                    let id_pin = row.id.clone();
                                    let row_set = row.clone();
                                    let id_rev = row.id.clone();
                                    let id_comp = row.id.clone();
                                    let row_share = row.clone();
                                    let row_copy_md = row.clone();
                                    let row_dl_md = row.clone();
                                    let row_link = row.clone();
                                    let id_arch = row.id.clone();
                                    let row_del = row.clone();
                                    vec![
                                        MenuAction::new("Find in thread", "search").run(move || find_open.set(true)),
                                        MenuAction::new("Rename thread", "pencil").run(move || { nav.set(true); start_rename.run(row_ren.clone()); }),
                                        MenuAction::new(if pinned_now { "Unpin thread" } else { "Pin thread" }, "pin").disabled(!loaded_now).run(move || toggle_pin.run(id_pin.clone())),
                                        MenuAction::new(if settled_now { "Restore thread" } else { "Settle thread" }, "check").disabled(settling.get_untracked() || !loaded_now || working_now).run(move || settle.run(row_set.clone())),
                                        MenuAction::new("Review changes with the agent", "check").disabled(working_now || !connected).run(move || dispatch_agent_special(&id_rev, "review")),
                                        MenuAction::new("Compact context", "list").disabled(working_now || !connected).run(move || dispatch_agent_special(&id_comp, "compact")),
                                        MenuAction::new("Share\u{2026}", "user").run(move || sharing.set(Some(row_share.clone()))),
                                        MenuAction::new("Copy as Markdown", "copy").run(move || export_thread.run((row_copy_md.clone(), "copy"))),
                                        MenuAction::new("Export as Markdown", "download").run(move || export_thread.run((row_dl_md.clone(), "download"))),
                                        MenuAction::new("Copy thread link", "share").run(move || copy_thread.run(row_link.clone())),
                                        MenuAction::new("Archive thread", "archive").disabled(!loaded_now || working_now).run(move || toggle_archive.run(id_arch.clone())),
                                        MenuAction::new("Delete thread", "trash").disabled(working_now).run(move || { delete_error.set(String::new()); deleting.set(Some(row_del.clone())); }),
                                    ]
                                })><Icon name="more"/></ActionMenu>
                                <div class="agent-panel-controls">
                                    <ActionMenu label="Open surface" actions=Callback::new(move |_| {
                                        vec![
                                            MenuAction::new("Changes (Diff)", "diff").run(move || open_surface.run(SurfaceKind::Diff)),
                                            MenuAction::new("Workspace files", "folder").run(move || open_surface.run(SurfaceKind::Files)),
                                            MenuAction::new("Side chat", "chat").run(move || open_surface.run(SurfaceKind::SideChat)),
                                            MenuAction::new("PRs & Issues", "git").run(move || open_surface.run(SurfaceKind::PullRequests)),
                                            MenuAction::new("Workspace preview", "globe").run(move || open_surface.run(SurfaceKind::Preview)),
                                            MenuAction::new("Interactive terminal", "terminal").run(move || { drawer_tab.set("shell"); terminal.set(true); }),
                                        ]
                                    })><Icon name="grid"/></ActionMenu>
                                    <button class="icon-button agent-desktop-toggle" aria-label="Toggle thread details" aria-expanded=move || details.get().to_string() on:click=move |_| details.update(|v| *v = !*v)><Icon name="info"/></button>
                                    <button class="icon-button agent-desktop-toggle" aria-label="Toggle command output" aria-expanded=move || terminal.get().to_string() on:click=move |_| terminal.update(|v| *v = !*v)><Icon name="list"/></button>
                                    <button class="icon-button agent-desktop-toggle" aria-label=move || if review.get() { "Hide changes" } else { "Show changes" } aria-expanded=move || review.get().to_string() on:click=move |_| if review.get_untracked() { close_review.run(()); } else { open_review.run(()); }><Icon name="file"/></button>
                                </div>
                            }
                        })}
                    </header>
                    {move || { let e = run_error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
                    {move || { let e = layout_error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
                    <div class="agent-stage">
                        <div class="agent-chat">
                            {move || match active_page.get() {
                                AgentPage::Usage => {
                                    let provs_sig = Signal::derive(move || {
                                        provider.with(|p| {
                                            p.as_ref()
                                                .and_then(|p| p.providers.as_ref())
                                                .map(|list| {
                                                    list.iter()
                                                        .map(|i| PageProviderInfo {
                                                            id: i.id.clone(),
                                                            name: i.name.clone(),
                                                            configured: i.configured,
                                                            connected: i.connected,
                                                            message: i.message.clone(),
                                                        })
                                                        .collect::<Vec<_>>()
                                                })
                                                .unwrap_or_else(|| vec![PageProviderInfo {
                                                    id: "codex".into(),
                                                    name: "Codex".into(),
                                                    configured: p.as_ref().is_some_and(|x| x.configured),
                                                    connected: p.as_ref().is_some_and(|x| x.connected),
                                                    message: p.as_ref().map(|x| x.message.clone()).unwrap_or_default(),
                                                }])
                                        })
                                    });
                                    let models_sig = Signal::derive(move || {
                                        provider.with(|p| {
                                            p.as_ref()
                                                .map(|p| {
                                                    p.models
                                                        .iter()
                                                        .map(|m| PageModelInfo {
                                                            id: m.id.clone(),
                                                            name: m.name.clone(),
                                                            provider: provider_display_name(m.provider.as_deref().unwrap_or("codex")).to_owned(),
                                                            is_default: m.is_default,
                                                            efforts: m.efforts.clone().unwrap_or_default(),
                                                        })
                                                        .collect::<Vec<_>>()
                                                })
                                                .unwrap_or_default()
                                        })
                                    });
                                    let lims_sig = Signal::derive(move || {
                                        limits.with(|opt| {
                                            opt.as_ref().map(|l| PageLimits {
                                                available: l.available,
                                                plan: l.plan.clone(),
                                                windows: l.windows.iter().map(|w| PageLimitWindow {
                                                    label: w.label.clone(),
                                                    used_percent: w.used_percent,
                                                    resets_at: w.resets_at,
                                                }).collect(),
                                                message: l.message.clone(),
                                            })
                                        })
                                    });
                                    let threads_sig = Signal::derive(move || {
                                        threads.with(|t| {
                                            t.as_ref()
                                                .map(|list| list.iter().map(|r| PageThreadInfo {
                                                    id: r.id.clone(),
                                                    project_id: r.project_id.clone(),
                                                    title: r.title.clone(),
                                                    modified_at: r.modified_at,
                                                }).collect::<Vec<_>>())
                                                .unwrap_or_default()
                                        })
                                    });
                                    view! {
                                        <UsagePage
                                            providers=provs_sig
                                            models=models_sig
                                            limits=lims_sig
                                            threads=threads_sig
                                            starred_models=starred_models
                                            checking=checking_provider
                                            is_admin=is_admin
                                            on_refresh=Callback::new(move |_| provider_epoch.update(|n| *n += 1))
                                            on_connect=connect_provider
                                            on_toggle_star=toggle_star_model
                                            on_back=Callback::new(move |_| active_page.set(AgentPage::Workspace))
                                        />
                                    }.into_any()
                                }
                                AgentPage::PullRequests => view! {
                                    <PullRequestsPage
                                        on_start_thread_prompt=Callback::new(move |prompt: String| {
                                            prefill_draft.set(Some(prompt));
                                            active_page.set(AgentPage::Workspace);
                                            choose_new_thread.run(());
                                        })
                                        on_back=Callback::new(move |_| active_page.set(AgentPage::Workspace))
                                    />
                                }.into_any(),
                                AgentPage::Automations => {
                                    let projs_sig = Signal::derive(move || {
                                        projects.with(|p| {
                                            p.as_ref()
                                                .map(|list| list.iter().map(|i| PageProjectInfo {
                                                    id: i.id.clone(),
                                                    name: i.name.clone(),
                                                    source_type: i.source_type.clone(),
                                                }).collect::<Vec<_>>())
                                                .unwrap_or_default()
                                        })
                                    });
                                    view! {
                                        <AutomationsPage
                                            schedules=schedules
                                            projects=projs_sig
                                            on_save=update_schedules
                                            on_run_now=Callback::new(move |(pid, prompt): (String, String)| {
                                                prefill_draft.set(Some(prompt));
                                                active_page.set(AgentPage::Workspace);
                                                if !pid.is_empty() {
                                                    project_id.set(pid);
                                                    thread_id.set(String::new());
                                                    view.set(View::Conversation);
                                                } else if projects.with_untracked(|p| p.as_ref().is_none_or(Vec::is_empty)) {
                                                    start_scratch.run(());
                                                } else {
                                                    choose_new_thread.run(());
                                                }
                                            })
                                            on_back=Callback::new(move |_| active_page.set(AgentPage::Workspace))
                                        />
                                    }.into_any()
                                }
                                AgentPage::Workspace => match chat_slot.get() {
                                ChatSlot::Shared(_) => {
                                    let initial = shared_item.get_untracked().expect("shared_item present for ChatSlot::Shared");
                                    let sig = Signal::derive(move || shared_item.get().unwrap_or_else(|| initial.clone()));
                                    view! { <SharedConversation item=sig/> }.into_any()
                                }
                                ChatSlot::Thread(tid_key) => {
                                    let Some(t) = thread.get_untracked().filter(|t| t.id == tid_key) else {
                                        return ().into_any();
                                    };
                                    let tid_start = t.id.clone();
                                    let auto = auto_start.with_value(|m| m.borrow().get(&tid_start).copied());
                                    let init_files = start_files.with_value(|m| m.borrow().get(&tid_start).cloned());
                                    let tid_clear = t.id.clone();
                                    view! {
                                        <Conversation
                                            thread=t
                                            project=project
                                            session=session_sv.get_value()
                                            on_changed=changed
                                            draft_cache=draft_cache
                                            positions=positions
                                            provider=provider
                                            checking_provider=checking_provider
                                            run=run
                                            on_run=Callback::new(move |id| run_id.set(id))
                                            on_stop=stop
                                            on_review=open_review
                                            history_epoch=history_epoch
                                            find_open=find_open
                                            on_close_find=Callback::new(move |_| find_open.set(false))
                                            auto_start=auto
                                            on_started=Callback::new(move |_| {
                                                auto_start.with_value(|m| { m.borrow_mut().remove(&tid_clear); });
                                                start_files.with_value(|m| { m.borrow_mut().remove(&tid_clear); });
                                            })
                                            send_key=send_key
                                            follow_up_mode=follow_up_mode
                                            starred_models=starred_models
                                            on_toggle_star=toggle_star_model
                                            context_chips=context_chips
                                            stash=prompt_stash
                                            on_update_stash=update_stash
                                            on_open_file=open_file_in_surface
                                            incoming_draft=incoming_draft
                                            commands=slash_commands
                                            initial_files=init_files
                                            on_forked=Callback::new(move |row: AgentThread| {
                                                let rid = row.id.clone();
                                                let pid = row.project_id.clone();
                                                threads.update(|cur| {
                                                    let mut next = vec![row];
                                                    if let Some(existing) = cur.take() {
                                                        next.extend(existing.into_iter().filter(|i| i.id != rid));
                                                    }
                                                    *cur = Some(next);
                                                });
                                                thread_id.set(rid);
                                                project_id.set(pid);
                                                view.set(View::Conversation);
                                            })
                                        />
                                    }.into_any()
                                }
                                ChatSlot::Draft(pid_key) => {
                                    let Some(p) = project.get_untracked().filter(|p| p.id == pid_key) else {
                                        return ().into_any();
                                    };
                                    view! {
                                        <DraftComposer
                                            session=session_sv.get_value()
                                            project=p
                                            provider=provider
                                            checking_provider=checking_provider
                                            draft_cache=draft_cache
                                            on_choose_project=Callback::new(move |_| editor.set(Some("thread")))
                                            on_start_scratch=start_scratch
                                            on_created=started
                                            send_key=send_key
                                            starred_models=starred_models
                                            on_toggle_star=toggle_star_model
                                            prefill_prompt=prefill_draft
                                            commands=slash_commands
                                        />
                                    }.into_any()
                                }
                                ChatSlot::Empty(loading_projects) => view! {
                                    <div class="agent-empty agent-hero">
                                        {if loading_projects {
                                            view! { <p role="status">"Loading projects\u{2026}"</p> }.into_any()
                                        } else {
                                            view! {
                                                <h2>"Add a project to start"</h2>
                                                <p>"Work in a Git repository, a folder in Dar Files, or an instant scratch workspace."</p>
                                                <div class="agent-empty-actions">
                                                    <button class="primary-button" on:click=move |_| editor.set(Some("project"))><Icon name="plus"/>"Add project"</button>
                                                    <button type="button" class="secondary-button" on:click=move |_| start_scratch.run(())>"Start without a project"</button>
                                                </div>
                                            }.into_any()
                                        }}
                                    </div>
                                }.into_any(),
                                },
                            }}
                        </div>
                        {move || has_thread.get().then(|| {
                            let tid_git = Signal::derive(move || thread_id.get());
                            view! {
                                <aside class="agent-context" data-agent-panel="details" tabindex="-1" aria-label="Thread details">
                                    <div class="agent-context-card">
                                        {move || {
                                            let p = project.get();
                                            let pname = p.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "Project".into());
                                            let is_files = p.as_ref().is_some_and(|p| p.source_type == "files");
                                            let source_ref = p.as_ref().map(|p| p.source_ref.clone()).unwrap_or_default();
                                            let has_project = p.is_some();
                                            view! {
                                                <div class="agent-context-project">
                                                    <ProjectMark name=pname.clone()/>
                                                    <strong>{pname}</strong>
                                                    <span>{if is_files { "Dar Files" } else { "Git" }}</span>
                                                </div>
                                                {if is_files {
                                                    view! { <a href=format!("/?folder={source_ref}")><Icon name="folder"/>"Open folder in Files"<Icon name="chevron"/></a> }.into_any()
                                                } else if has_project {
                                                    view! { <div class="agent-source-ref"><Icon name="share"/><code>{source_ref}</code></div> }.into_any()
                                                } else {
                                                    ().into_any()
                                                }}
                                            }
                                        }}
                                        <div class="agent-workspace-label"><Icon name="lock"/>"Isolated working copy"</div>
                                        <ProjectScriptsCard
                                            project_id=Signal::derive(move || project.with(|p| p.as_ref().map(|p| p.id.clone()).unwrap_or_default()))
                                            scripts=Signal::derive(move || {
                                                let pid = project.with(|p| p.as_ref().map(|p| p.id.clone()).unwrap_or_default());
                                                project_scripts_map.with(|m| {
                                                    m.get(&pid)
                                                        .and_then(|v| serde_json::from_value::<Vec<ProjectScript>>(v.clone()).ok())
                                                        .unwrap_or_default()
                                                })
                                            })
                                            on_save_scripts=Callback::new(move |next: Vec<ProjectScript>| {
                                                let pid = project.with_untracked(|p| p.as_ref().map(|p| p.id.clone()).unwrap_or_default());
                                                if pid.is_empty() {
                                                    return;
                                                }
                                                let patch = scripts_patch(project_scripts_map.get_untracked(), &pid, next);
                                                update_preferences.run(patch);
                                            })
                                            on_run_script=Callback::new(move |script: ProjectScript| {
                                                let next_seq = script_seq.get_untracked() + 1;
                                                script_seq.set(next_seq);
                                                script_launch.set(Some(ScriptLaunch {
                                                    seq: next_seq,
                                                    name: script.name,
                                                    command: script.command,
                                                }));
                                                drawer_tab.set("shell");
                                                terminal.set(true);
                                                if is_narrow() {
                                                    view.set(View::Terminal);
                                                }
                                            })
                                        />
                                        <GitCard thread_id=tid_git session=session_sv.get_value() epoch=history_epoch/>
                                        <button on:click=move |_| open_review.run(())>
                                            <Icon name="file"/>"Changes"
                                            {move || {
                                                let counts = run.with(|r| {
                                                    r.as_ref().filter(|r| !r.document.changes.is_empty()).map(|r| {
                                                        r.document.changes.iter().fold((0usize, 0usize), |(a, rem), c| {
                                                            let (da, dr) = diff_counts(&c.diff);
                                                            (a + da, rem + dr)
                                                        })
                                                    })
                                                });
                                                match counts {
                                                    Some((a, rem)) => view! { <DiffStat added=a removed=rem/> }.into_any(),
                                                    None => view! { <span>"0"</span> }.into_any(),
                                                }
                                            }}
                                            <Icon name="chevron"/>
                                        </button>
                                        {move || run.with(|r| r.as_ref().and_then(|r| r.document.usage.clone())).map(|u| view! {
                                            <div class="agent-context-usage" title="Tokens reported by the agent CLI for the latest turn">
                                                <Icon name="info"/>"Latest turn"<span>{format!("{} in \u{00b7} {} out", tokens(u.input), tokens(u.output))}</span>
                                            </div>
                                        })}
                                        {move || run.with(|r| r.as_ref().and_then(|r| r.document.effort.clone())).filter(|e| !e.is_empty()).map(|eff| view! {
                                            <div class="agent-context-usage"><Icon name="agent"/>"Reasoning"<span>{effort_label(&eff)}</span></div>
                                        })}
                                        {move || limits.get().filter(|l| l.available).map(|l| {
                                            l.windows.into_iter().take(2).map(|w| view! {
                                                <div class="agent-context-usage agent-limit" title=w.resets_at.map(|ts| format!("Resets {}", locale_string(ts)))>
                                                    <Icon name="clock"/>{w.label}<span>{format!("{}%", w.used_percent.round() as i64)}</span>
                                                </div>
                                            }).collect_view()
                                        })}
                                        <div class="agent-context-provider">
                                            {move || {
                                                let connected = provider.with(|p| p.as_ref().is_some_and(|p| p.connected));
                                                let checking = checking_provider.get();
                                                let label = if checking {
                                                    "Checking agents\u{2026}".into()
                                                } else if connected {
                                                    let names = provider.with(|p| {
                                                        p.as_ref()
                                                            .and_then(|p| p.providers.as_ref())
                                                            .map(|list| list.iter().filter(|i| i.connected).map(|i| i.name.as_str()).collect::<Vec<_>>().join(" + "))
                                                            .filter(|s| !s.is_empty())
                                                            .unwrap_or_else(|| "Agent".into())
                                                    });
                                                    format!("{names} connected")
                                                } else {
                                                    "Agent unavailable".into()
                                                };
                                                view! {
                                                    <span class=if connected { "agent-connected" } else { "" }/>
                                                    {label}
                                                }
                                            }}
                                            <button class="icon-button" aria-label="Connection settings" on:click=move |_| settings.set(true)><Icon name="settings"/></button>
                                        </div>
                                    </div>
                                </aside>
                            }
                        })}
                    </div>
                    <section class=move || if terminal.get() { "agent-terminal is-open" } else { "agent-terminal" } data-agent-panel="terminal" tabindex="-1" aria-label="Terminal output">
                        <div class="agent-output-heading">
                            <div role="tablist" aria-label="Output" class="agent-drawer-tabs">
                                <button role="tab" aria-selected=move || (drawer_tab.get() == "output").to_string() on:click=move |_| drawer_tab.set("output")>"Agent output"</button>
                                <button role="tab" aria-selected=move || (drawer_tab.get() == "shell").to_string() disabled=move || thread.with(Option::is_none) on:click=move |_| drawer_tab.set("shell")>"Terminal"</button>
                            </div>
                            {move || (drawer_tab.get() == "output").then(|| view! { <span class="muted">"Read only"</span> })}
                            <button class="icon-button" aria-label="Close command output" on:click=move |_| { terminal.set(false); view.set(View::Conversation); }><Icon name="close"/></button>
                        </div>
                        {move || has_thread.get().then(|| {
                            let tid_sig = Signal::derive(move || thread_id.get());
                            let vis_sig = Signal::derive(move || terminal.get() && drawer_tab.get() == "shell");
                            view! {
                                <div class="agent-drawer-panel" hidden=move || drawer_tab.get() != "shell">
                                    <MultiTerminalDrawer
                                        thread_id=tid_sig
                                        session=session_sv.get_value()
                                        visible=vis_sig
                                        script_launch=script_launch
                                        on_send_to_composer=Callback::new(move |text: String| {
                                            attach_chip.run(ContextChip {
                                                id: random_uuid(),
                                                label: "Terminal output".into(),
                                                kind: "terminal",
                                                content: format!("Terminal output:\n```\n{text}\n```"),
                                            });
                                        })
                                    />
                                </div>
                            }
                        })}
                        {
                            let terminal_output = Memo::new(move |_| {
                                if drawer_tab.get() != "output" {
                                    return None;
                                }
                                Some(run.with(|cur| {
                                    let cmd_steps: Vec<AgentStep> = cur
                                        .as_ref()
                                        .and_then(|r| r.document.steps.as_ref())
                                        .map(|s| s.iter().filter(|st| st.kind == "command").cloned().collect())
                                        .unwrap_or_default();
                                    let term_txt = cur
                                        .as_ref()
                                        .map(|r| r.document.terminal.clone())
                                        .filter(|t| !t.is_empty());
                                    (cmd_steps, term_txt)
                                }))
                            });
                            move || {
                                let Some((cmd_steps, term_txt)) = terminal_output.get() else {
                                    return ().into_any();
                                };
                                if !cmd_steps.is_empty() {
                                    view! {
                                        <ol class="agent-terminal-steps">
                                            {cmd_steps.into_iter().map(|step| {
                                                let st_label = if step.status == "running" {
                                                    "running".into()
                                                } else if let Some(code) = step.exit_code {
                                                    format!("exit {code}")
                                                } else {
                                                    step.status.clone()
                                                };
                                                let out = step.output.filter(|o| !o.is_empty());
                                                view! {
                                                    <li data-status=step.status>
                                                        <div><span class="agent-terminal-prompt">"$"</span><code>{step.label}</code><span class="agent-terminal-state">{st_label}</span></div>
                                                        {out.map(|o| view! { <pre>{o}</pre> })}
                                                    </li>
                                                }
                                            }).collect_view()}
                                        </ol>
                                    }.into_any()
                                } else if let Some(term_txt) = term_txt {
                                    view! { <pre>{term_txt}</pre> }.into_any()
                                } else {
                                    view! { <p>"No command output yet."</p> }.into_any()
                                }
                            }
                        }
                    </section>
                </main>
                <aside class="agent-review" data-agent-panel="changes" tabindex="-1" aria-label="Changes">
                    <Resizer label="Resize changes" side="left" value=review_width min=340.0 max=1200.0 on_change=Callback::new(move |v| review_width.set(v))/>
                    <header class="agent-review-heading">
                        <div class="agent-surface-tabs" role="tablist" aria-label="Workspace surfaces">
                            {SurfaceKind::all().into_iter().map(|surf| {
                                view! {
                                    <button type="button" role="tab"
                                        class=move || if active_surface.get() == surf { "agent-surface-tab is-active" } else { "agent-surface-tab" }
                                        aria-selected=move || (active_surface.get() == surf).to_string()
                                        on:click=move |_| active_surface.set(surf)>
                                        <Icon name=surf.icon()/>
                                        <span>{surf.label()}</span>
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                        <button class="icon-button agent-desktop-toggle" aria-label=move || if maximized.get() { "Restore review size" } else { "Maximize review" } on:click=move |_| maximized.update(|v| *v = !*v)><Icon name="grid"/></button>
                        <button class="icon-button" aria-label="Close changes" on:click=move |_| close_review.run(())><Icon name="close"/></button>
                    </header>
                    <div class="agent-review-content">
                        {move || match active_surface.get() {
                            SurfaceKind::Diff => {
                                review_run_id.track();
                                view! {
                                    <ChangeReview
                                        run=run
                                        session=session_sv.get_value()
                                        default_diff_view=diff_view
                                        on_attach_chip=attach_chip
                                        on_reverted=Callback::new(move |_| history_epoch.update(|n| *n += 1))
                                    />
                                }.into_any()
                            }
                            SurfaceKind::Files => view! {
                                <FilesSurface
                                    thread_id=Signal::derive(move || thread_id.get())
                                    initial_file=surface_file
                                    on_attach_chip=attach_chip
                                    on_open_preview=Callback::new(move |path: String| {
                                        preview_file.set(Some(path));
                                        active_surface.set(SurfaceKind::Preview);
                                    })
                                />
                            }.into_any(),
                            SurfaceKind::SideChat => view! {
                                <SideChatSurface
                                    thread_id=Signal::derive(move || thread_id.get())
                                    on_attach_chip=attach_chip
                                    on_send_prompt=Callback::new(move |text: String| {
                                        incoming_draft.set(Some(text));
                                    })
                                />
                            }.into_any(),
                            SurfaceKind::PullRequests => view! {
                                <PullRequestsSurface on_attach_chip=attach_chip/>
                            }.into_any(),
                            SurfaceKind::Preview => view! {
                                <PreviewSurface
                                    thread_id=Signal::derive(move || thread_id.get())
                                    target_file=preview_file
                                />
                            }.into_any(),
                        }}
                    </div>
                </aside>
            </div>

            {move || (editor.get() == Some("project")).then(|| view! {
                <ProjectEditor
                    session=session_sv.get_value()
                    on_close=Callback::new(move |_| {
                        let has_projs = projects.with_untracked(|p| p.as_ref().is_some_and(|v| !v.is_empty()));
                        editor.set(if has_projs { Some("thread") } else { None });
                    })
                    on_created=Callback::new(move |row: AgentProject| {
                        let pid = row.id.clone();
                        projects.update(|cur| {
                            let mut next = cur.take().unwrap_or_default();
                            next.push(row);
                            *cur = Some(next);
                        });
                        project_id.set(pid);
                        thread_id.set(String::new());
                        editor.set(None);
                        view.set(View::Conversation);
                    })
                />
            })}

            {move || (editor.get() == Some("thread") && has_projects.get()).then(|| {
                let projs_sig = Signal::derive(move || projects.get().unwrap_or_default());
                view! {
                    <ThreadChooser
                        projects=projs_sig
                        busy=Signal::derive(|| false)
                        error=create_error
                        on_close=Callback::new(move |_| editor.set(None))
                        on_choose=Callback::new(move |row: AgentProject| {
                            project_id.set(row.id);
                            thread_id.set(String::new());
                            editor.set(None);
                            view.set(View::Conversation);
                        })
                        on_add=Callback::new(move |_| editor.set(Some("project")))
                    />
                }
            })}

            {move || settings.get().then(|| {
                let provs_sig = Signal::derive(move || {
                    provider.with(|p| {
                        p.as_ref()
                            .and_then(|p| p.providers.as_ref())
                            .map(|list| {
                                list.iter()
                                    .map(|i| PageProviderInfo {
                                        id: i.id.clone(),
                                        name: i.name.clone(),
                                        configured: i.configured,
                                        connected: i.connected,
                                        message: i.message.clone(),
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_else(|| vec![PageProviderInfo {
                                id: "codex".into(),
                                name: "Codex".into(),
                                configured: p.as_ref().is_some_and(|x| x.configured),
                                connected: p.as_ref().is_some_and(|x| x.connected),
                                message: p.as_ref().map(|x| x.message.clone()).unwrap_or_default(),
                            }])
                    })
                });
                let models_sig = Signal::derive(move || {
                    provider.with(|p| {
                        p.as_ref()
                            .map(|p| {
                                p.models
                                    .iter()
                                    .map(|m| PageModelInfo {
                                        id: m.id.clone(),
                                        name: m.name.clone(),
                                        provider: provider_display_name(m.provider.as_deref().unwrap_or("codex")).to_owned(),
                                        is_default: m.is_default,
                                        efforts: m.efforts.clone().unwrap_or_default(),
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                });
                let lims_sig = Signal::derive(move || {
                    limits.with(|opt| {
                        opt.as_ref().map(|l| PageLimits {
                            available: l.available,
                            plan: l.plan.clone(),
                            windows: l.windows.iter().map(|w| PageLimitWindow {
                                label: w.label.clone(),
                                used_percent: w.used_percent,
                                resets_at: w.resets_at,
                            }).collect(),
                            message: l.message.clone(),
                        })
                    })
                });
                let archived_sig = Signal::derive(move || {
                    let arch_ids = archived.get();
                    let proj_list = projects.get().unwrap_or_default();
                    threads
                        .get()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|r| arch_ids.contains(&r.id))
                        .map(|r| {
                            let pname = proj_list
                                .iter()
                                .find(|p| p.id == r.project_id)
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| "Project".into());
                            (
                                PageThreadInfo {
                                    id: r.id,
                                    project_id: r.project_id,
                                    title: r.title,
                                    modified_at: r.modified_at,
                                },
                                pname,
                            )
                        })
                        .collect::<Vec<_>>()
                });
                view! {
                    <AgentSettingsModal
                        section=settings_section
                        notice=Signal::from(settings_notice)
                        providers=provs_sig
                        models=models_sig
                        limits=lims_sig
                        checking_provider=checking_provider
                        connection_error=connection_error
                        send_key=send_key
                        follow_up_mode=follow_up_mode
                        auto_settle=auto_settle
                        diff_view=diff_view
                        grouping=grouping
                        notify_enabled=notify_enabled
                        permission=permission
                        prefs_loaded=prefs_loaded
                        archived_threads=archived_sig
                        is_admin=is_admin
                        user_id=user_id_sv.get_value()
                        on_close=Callback::new(move |_| settings.set(false))
                        on_refresh_provider=Callback::new(move |_| provider_epoch.update(|n| *n += 1))
                        on_connect_provider=connect_provider
                        on_update_prefs=update_preferences
                        on_request_notify=Callback::new(move |enabled: bool| {
                            if enabled && notification_permission() == "default" {
                                spawn_local(async move {
                                    let res = request_notification_permission().await;
                                    permission.try_set(res.clone());
                                    if res == "granted" {
                                        let mut map = serde_json::Map::new();
                                        map.insert("agentNotify".into(), Value::Bool(true));
                                        update_preferences.run(map);
                                    }
                                });
                            } else {
                                let mut map = serde_json::Map::new();
                                map.insert("agentNotify".into(), Value::Bool(enabled));
                                update_preferences.run(map);
                            }
                        })
                        on_unarchive=toggle_archive
                        on_delete_archived=Callback::new(move |tid: String| {
                            if let Some(row) = threads.with_untracked(|t| t.as_ref().and_then(|l| l.iter().find(|x| x.id == tid).cloned())) {
                                delete_error.set(String::new());
                                deleting.set(Some(row));
                            }
                        })
                    />
                }
            })}

            {move || palette.get().then(|| {
                let mut items = vec![
                    PaletteItem { group: "Actions", label: "New thread".into(), detail: Some("Ctrl Shift O".into()), mark: None, icon: Some("plus"), run: Callback::new(move |_| choose_new_thread.run(())) },
                    PaletteItem { group: "Actions", label: "Start without a project".into(), detail: Some("Scratch".into()), mark: None, icon: Some("chat"), run: Callback::new(move |_| start_scratch.run(())) },
                    PaletteItem { group: "Actions", label: "Add project".into(), detail: None, mark: None, icon: Some("folder"), run: Callback::new(move |_| editor.set(Some("project"))) },
                    PaletteItem { group: "Pages", label: "Usage & limits".into(), detail: None, mark: None, icon: Some("chart"), run: Callback::new(move |_| active_page.set(AgentPage::Usage)) },
                    PaletteItem { group: "Pages", label: "Pull requests & issues".into(), detail: None, mark: None, icon: Some("git"), run: Callback::new(move |_| active_page.set(AgentPage::PullRequests)) },
                    PaletteItem { group: "Pages", label: "Automations".into(), detail: None, mark: None, icon: Some("clock"), run: Callback::new(move |_| active_page.set(AgentPage::Automations)) },
                ];
                if let Some(t) = thread.get_untracked() {
                    let t_ren = t.clone();
                    let id_pin = t.id.clone();
                    let id_arch = t.id.clone();
                    let is_pin = pinned.with_untracked(|p| p.contains(&t.id));
                    let rev_open = review.get_untracked();
                    let term_open = terminal.get_untracked();
                    items.extend([
                        PaletteItem { group: "Actions", label: "Find in thread".into(), detail: Some("Ctrl F".into()), mark: None, icon: Some("search"), run: Callback::new(move |_| find_open.set(true)) },
                        PaletteItem { group: "Actions", label: "Rename thread".into(), detail: None, mark: None, icon: Some("pencil"), run: Callback::new(move |_| start_rename.run(t_ren.clone())) },
                        PaletteItem { group: "Actions", label: if is_pin { "Unpin thread".into() } else { "Pin thread".into() }, detail: None, mark: None, icon: Some("pin"), run: Callback::new(move |_| toggle_pin.run(id_pin.clone())) },
                        PaletteItem { group: "Actions", label: "Archive thread".into(), detail: None, mark: None, icon: Some("archive"), run: Callback::new(move |_| toggle_archive.run(id_arch.clone())) },
                        PaletteItem { group: "Actions", label: if rev_open { "Hide changes".into() } else { "Show changes".into() }, detail: None, mark: None, icon: Some("file"), run: Callback::new(move |_| if review.get_untracked() { close_review.run(()); } else { open_review.run(()); }) },
                        PaletteItem { group: "Actions", label: if term_open { "Hide command output".into() } else { "Show command output".into() }, detail: None, mark: None, icon: Some("terminal"), run: Callback::new(move |_| terminal.update(|v| *v = !*v)) },
                    ]);
                }
                let nav_open = nav.get_untracked();
                items.extend([
                    PaletteItem { group: "Actions", label: if nav_open { "Hide threads".into() } else { "Show threads".into() }, detail: Some("Ctrl B".into()), mark: None, icon: Some("sidebar"), run: Callback::new(move |_| nav.update(|v| *v = !*v)) },
                    PaletteItem { group: "Actions", label: "Agent settings".into(), detail: None, mark: None, icon: Some("settings"), run: Callback::new(move |_| settings.set(true)) },
                ]);
                let proj_list = projects.get_untracked().unwrap_or_default();
                let arch_ids = archived.get_untracked();
                for row in threads.get_untracked().unwrap_or_default().into_iter().filter(|r| !arch_ids.contains(&r.id)) {
                    let pname = proj_list.iter().find(|p| p.id == row.project_id).map(|p| p.name.clone());
                    let rid = row.id.clone();
                    let pid = row.project_id.clone();
                    items.push(PaletteItem {
                        group: "Threads",
                        label: row.title,
                        detail: pname.clone(),
                        mark: Some(pname.unwrap_or_else(|| "Project".into())),
                        icon: None,
                        run: Callback::new(move |_| {
                            thread_id.set(rid.clone());
                            project_id.set(pid.clone());
                            view.set(View::Conversation);
                        }),
                    });
                }
                for row in proj_list {
                    let pid = row.id.clone();
                    items.push(PaletteItem {
                        group: "New thread in project",
                        label: row.name.clone(),
                        detail: Some(if row.source_type == "files" { "Dar Files".into() } else { "Git".into() }),
                        mark: Some(row.name),
                        icon: None,
                        run: Callback::new(move |_| {
                            thread_id.set(String::new());
                            project_id.set(pid.clone());
                            view.set(View::Conversation);
                            request_animation_frame(|| {
                                if let Some(el) = document().get_element_by_id("agent-message").and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
                                    let _ = el.focus();
                                }
                            });
                        }),
                    });
                }
                view! { <CommandPalette items=items on_close=Callback::new(move |_| palette.set(false))/> }
            })}

            {move || sharing.get().map(|t| view! {
                <ShareDialog thread=t session=session_sv.get_value() on_close=Callback::new(move |_| sharing.set(None))/>
            })}

            {move || snoozing.get().map(|t| {
                let tid = StoredValue::new(t.id);
                let min_dt = StoredValue::new(local_datetime_min_now());
                view! {
                    <Dialog title="Snooze thread" on_close=Callback::new(move |_| snoozing.set(None))>
                        <form class="agent-git-form" on:submit=move |e| {
                            e.prevent_default();
                            let until = parse_datetime_local(&snooze_at.get_untracked());
                            if until > js_sys::Date::now() {
                                snooze_thread.run((tid.get_value(), Some(until)));
                                snoozing.set(None);
                            }
                        }>
                            <label>
                                "Wake at"
                                <input type="datetime-local" required autofocus prop:value=move || snooze_at.get() min=min_dt.get_value()
                                    on:input=move |e| snooze_at.set(event_target_value(&e))/>
                            </label>
                            <p class="muted small">
                                {move || if notify_enabled.get() {
                                    "The thread moves to Snoozed and returns at this time with a Woke badge and a notification."
                                } else {
                                    "The thread moves to Snoozed and returns at this time with a Woke badge."
                                }}
                            </p>
                            <div class="dialog-actions">
                                <button type="button" class="secondary-button" on:click=move |_| snoozing.set(None)>"Cancel"</button>
                                <button class="primary-button" disabled=move || {
                                    let v = snooze_at.get();
                                    v.is_empty() || !(parse_datetime_local(&v) > js_sys::Date::now())
                                }>"Snooze"</button>
                            </div>
                        </form>
                    </Dialog>
                }
            })}

            {move || deleting.get().map(|t| {
                let row_sv = StoredValue::new(t);
                view! {
                    <Dialog title="Delete thread" on_close=Callback::new(move |_| if !delete_busy.get_untracked() { deleting.set(None); })>
                        <div class="agent-delete">
                            <p>"Delete "<strong>{row_sv.with_value(|r| r.title.clone())}</strong>"? Its messages, run output and private working copy are removed. Edits already applied to Files are kept."</p>
                            {move || { let e = delete_error.get(); (!e.is_empty()).then(|| view! { <p role="alert" class="error">{e}</p> }) }}
                            <div class="dialog-actions">
                                <button class="secondary-button" disabled=move || delete_busy.get() on:click=move |_| deleting.set(None)>"Cancel"</button>
                                <button class="primary-button danger" disabled=move || delete_busy.get() on:click=move |_| remove_thread.run(row_sv.get_value())>
                                    {move || if delete_busy.get() { "Deleting\u{2026}" } else { "Delete thread" }}
                                </button>
                            </div>
                        </div>
                    </Dialog>
                }
            })}
        </div>
    }
}
