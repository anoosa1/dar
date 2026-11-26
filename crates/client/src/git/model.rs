//! Git app domain types, workspace state enums, URL/format helpers, preferences, and loaders.

use std::time::Duration;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

use super::diff::DiffMode;
use crate::api::{self, BrowserSession, Req};
use crate::shared::context;
use crate::ui::window;

// ---- Shared constants ---------------------------------------------------------------------------

pub const API: &str = "/api/v1/git";
pub const HISTORY_LIMITS_FILES: usize = 2000;
pub const HISTORY_LIMITS_DEPTH: usize = 32;
pub const HISTORY_LIMITS_REPOS: usize = 50;
pub const HISTORY_LIMITS_MESSAGE_BYTES: usize = 2048;

pub const GIT_LABEL_COLORS: [&str; 9] = [
    "plum", "violet", "blue", "teal", "green", "amber", "orange", "rose", "gray",
];

pub const TRACKER_LIMITS_TITLE: usize = 256;
pub const TRACKER_LIMITS_BODY: usize = 65_536;
pub const TRACKER_LIMITS_LABEL_NAME: usize = 50;
pub const TRACKER_LIMITS_LABEL_DESCRIPTION: usize = 200;
pub const TRACKER_LIMITS_LABELS_PER_ITEM: usize = 10;
pub const TRACKER_LIMITS_PAGE: usize = 500;

// ---- Workspace views, tabs, sheets & preferences ------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Overview,
    Repositories,
    Pulls,
    Agent,
}

impl View {
    pub fn as_str(self) -> &'static str {
        match self {
            View::Overview => "overview",
            View::Repositories => "repositories",
            View::Pulls => "pulls",
            View::Agent => "agent",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "repositories" => View::Repositories,
            "pulls" => View::Pulls,
            "agent" => View::Agent,
            _ => View::Overview,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            View::Overview => "Overview",
            View::Repositories => "Repositories",
            View::Pulls => "Pull requests",
            View::Agent => "Agent branches",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            View::Overview => "activity",
            View::Repositories => "repo",
            View::Pulls => "pull",
            View::Agent => "agent",
        }
    }
}

pub const VIEWS: [View; 4] = [View::Overview, View::Repositories, View::Pulls, View::Agent];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Code,
    Issues,
    Pulls,
    Commits,
    Branches,
    Tags,
}

impl Tab {
    pub fn as_str(self) -> &'static str {
        match self {
            Tab::Code => "code",
            Tab::Issues => "issues",
            Tab::Pulls => "pulls",
            Tab::Commits => "commits",
            Tab::Branches => "branches",
            Tab::Tags => "tags",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "issues" => Tab::Issues,
            "pulls" => Tab::Pulls,
            "commits" => Tab::Commits,
            "branches" => Tab::Branches,
            "tags" => Tab::Tags,
            _ => Tab::Code,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Code => "Code",
            Tab::Issues => "Issues",
            Tab::Pulls => "Pull requests",
            Tab::Commits => "Commits",
            Tab::Branches => "Branches",
            Tab::Tags => "Tags",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tab::Code => "code",
            Tab::Issues => "issue",
            Tab::Pulls => "pull",
            Tab::Commits => "commit",
            Tab::Branches => "branch",
            Tab::Tags => "tag",
        }
    }
}

pub const TABS: [Tab; 6] = [
    Tab::Code,
    Tab::Issues,
    Tab::Pulls,
    Tab::Commits,
    Tab::Branches,
    Tab::Tags,
];

#[derive(Clone, PartialEq, Eq)]
pub enum Sheet {
    Commit { sha: String },
    Compare { base: String, head: String },
    File { git_ref: String, path: String },
    Issue { number: u32, is_pull: bool },
}

#[derive(Clone)]
pub struct Notice {
    pub id: u64,
    pub message: String,
}

#[derive(Clone)]
pub struct RefLists {
    pub branches: GitBranches,
    pub tags: Vec<GitTag>,
}

#[derive(Clone)]
pub struct DeleteTarget {
    pub name: String,
    pub old: String,
    pub kind: &'static str,
    pub commits: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OverviewFilter {
    All,
    Dirty,
    Pulls,
    Shared,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OverviewSort {
    Updated,
    Name,
}

/// Account-wide Git UI preferences synchronized through `/api/v1/pim/preferences`.
#[derive(Clone, Copy)]
pub struct GitPrefs {
    pub diff_layout: Memo<DiffMode>,
    pub commits_view: Memo<String>,
    pub wrap_lines: Memo<bool>,
    pub show_about: Memo<bool>,
}

impl GitPrefs {
    pub fn from_context() -> Self {
        let prefs = context().preferences;
        let diff_layout = Memo::new(move |_| {
            prefs.value.with(|p| {
                let raw = p
                    .as_ref()
                    .and_then(|p| p.rest.get("gitDiffLayout"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("split");
                if raw == "unified" {
                    DiffMode::Unified
                } else {
                    DiffMode::Split
                }
            })
        });
        let commits_view = Memo::new(move |_| {
            prefs.value.with(|p| {
                p.as_ref()
                    .and_then(|p| p.rest.get("gitCommitsView"))
                    .and_then(serde_json::Value::as_str)
                    .map(|v| if v == "list" { "list" } else { "graph" })
                    .unwrap_or("graph")
                    .to_owned()
            })
        });
        let wrap_lines = Memo::new(move |_| {
            prefs.value.with(|p| {
                p.as_ref()
                    .and_then(|p| p.rest.get("gitWrapLines"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            })
        });
        let show_about = Memo::new(move |_| {
            prefs.value.with(|p| {
                p.as_ref()
                    .and_then(|p| p.rest.get("gitShowAbout"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true)
            })
        });
        Self {
            diff_layout,
            commits_view,
            wrap_lines,
            show_about,
        }
    }
}

// ---- Formatting and URL helpers -----------------------------------------------------------------

pub fn repo_api(id: &str) -> String {
    format!("{API}/repositories/{id}")
}

pub fn git_path(folder_id: &str) -> String {
    format!("/dav/git/{folder_id}.git")
}

pub fn encode_uri(value: &str) -> String {
    format!("{}", js_sys::encode_uri_component(value))
}

pub fn query_string(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                js_sys::encode_uri_component(k),
                js_sys::encode_uri_component(v)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

pub fn format_int(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

pub fn plural(count: usize, one: &str, many: Option<&str>) -> String {
    if count == 1 {
        format!("{} {one}", format_int(count))
    } else if let Some(many) = many {
        format!("{} {many}", format_int(count))
    } else {
        format!("{} {one}s", format_int(count))
    }
}

pub fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

pub fn source_label(source: &str) -> &'static str {
    match source {
        "git" => "Git push",
        "dar" => "Dar",
        "agent" => "Agent",
        _ => "Dar",
    }
}

pub fn source_icon(source: &str) -> &'static str {
    match source {
        "git" => "upload",
        "dar" => "folder",
        "agent" => "agent",
        _ => "folder",
    }
}

pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "added" => "Added",
        "modified" => "Modified",
        "deleted" => "Deleted",
        "renamed" => "Renamed",
        _ => "Modified",
    }
}

/// Deterministic repository color bucket (colour is data, not theme).
pub fn hue(text: &str) -> usize {
    let mut h: u64 = 0;
    for ch in text.chars() {
        h = (h * 31 + ch as u64) % 9973;
    }
    (h % 6) as usize
}

pub fn zip_url(id: &str, git_ref: &str) -> String {
    format!("{}/archive?ref={}", repo_api(id), encode_uri(git_ref))
}

pub fn strip_ref_prefix(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/tags/"))
        .unwrap_or(name)
}

pub fn is_markdown_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

pub fn is_valid_folder_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && trimmed != "."
        && trimmed != ".."
        && name.chars().count() <= 255
}

pub fn file_icon_and_class(kind: &str, name: &str) -> (&'static str, &'static str) {
    if kind == "tree" {
        return ("folder", "git-file-name is-tree");
    }
    if kind == "link" {
        return ("link", "git-file-name is-link");
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "py" | "go" | "c" | "h" | "cpp" | "java"
        | "sh" | "nix" | "html" | "css" | "sql" | "wasm" => ("code", "git-file-name is-code"),
        _ => ("file", "git-file-name is-doc"),
    }
}

pub fn file_language_label(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.eq_ignore_ascii_case("cargo.toml") {
        return "TOML";
    }
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "Rust",
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" | "mjs" => "JavaScript",
        "py" => "Python",
        "go" => "Go",
        "c" | "h" => "C",
        "cpp" | "cc" | "hpp" => "C++",
        "nix" => "Nix",
        "md" | "markdown" => "Markdown",
        "toml" => "TOML",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "css" => "CSS",
        "html" | "htm" => "HTML",
        "xml" | "svg" => "XML",
        "sh" | "bash" => "Shell",
        "txt" => "Text",
        _ => "Plain text",
    }
}

pub struct InitialUrlState {
    pub view: View,
    pub repo: Option<String>,
    pub tab: Tab,
    pub git_ref: String,
    pub path: String,
    pub commit: Option<String>,
    pub issue: Option<u32>,
}

pub fn read_url_state() -> InitialUrlState {
    let search = window().location().search().unwrap_or_default();
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok();
    let get = |key: &str| params.as_ref().and_then(|p| p.get(key));
    let view = get("view")
        .map(|v| View::from_str(&v))
        .unwrap_or(View::Overview);
    let tab = get("tab").map(|v| Tab::from_str(&v)).unwrap_or(Tab::Code);
    let issue = get("issue").and_then(|raw| {
        if !raw.is_empty()
            && raw.len() <= 6
            && raw.chars().all(|c| c.is_ascii_digit())
            && !raw.starts_with('0')
        {
            raw.parse::<u32>().ok()
        } else {
            None
        }
    });
    InitialUrlState {
        view,
        repo: get("repo"),
        tab,
        git_ref: get("ref").unwrap_or_default(),
        path: get("path").unwrap_or_default(),
        commit: get("commit"),
        issue,
    }
}

pub fn write_url_state(
    current_view: View,
    current_repo: Option<&str>,
    current_tab: Tab,
    current_ref: &str,
    current_path: &str,
    current_sheet: Option<&Sheet>,
) {
    if let Ok(href) = window().location().href()
        && let Ok(url) = web_sys::Url::new(&href)
    {
        let params = url.search_params();
        for key in ["view", "repo", "tab", "ref", "path", "commit", "issue"] {
            params.delete(key);
        }
        if let Some(id) = current_repo {
            params.set("repo", id);
            if current_tab != Tab::Code {
                params.set("tab", current_tab.as_str());
            }
            if !current_ref.is_empty() {
                params.set("ref", current_ref);
            }
            if !current_path.is_empty() {
                params.set("path", current_path);
            }
            match current_sheet {
                Some(Sheet::Commit { sha }) => params.set("commit", sha),
                Some(Sheet::Issue { number, .. }) => params.set("issue", &number.to_string()),
                _ => {}
            }
        } else if current_view != View::Overview {
            params.set("view", current_view.as_str());
        }
        if let Ok(history) = window().history() {
            let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url.href()));
        }
    }
}

// ---- History API shapes -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryChange {
    pub path: String,
    #[serde(default)]
    pub previous_path: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub previous_version_id: Option<String>,
    #[serde(default)]
    pub size: Option<f64>,
    #[serde(default)]
    pub previous_size: Option<f64>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub previous_media_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryChangeset {
    pub id: String,
    pub sequence: u64,
    pub kind: String,
    pub message: String,
    pub author: String,
    pub created_at: f64,
    pub file_count: usize,
    #[serde(default)]
    pub sha: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryBranch {
    pub name: String,
    pub thread_id: String,
    pub message: String,
    pub author: String,
    pub created_at: f64,
    pub modified_at: f64,
    #[serde(default)]
    pub changes: Vec<HistoryChange>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFolder {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRepo {
    pub revision: u64,
    pub head_id: String,
    pub sequence: u64,
    pub created_at: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryStatus {
    #[serde(default)]
    pub changes: Vec<HistoryChange>,
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryGitRef {
    pub name: String,
    pub sha: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOverview {
    pub folder: HistoryFolder,
    pub tracked: bool,
    pub can_write: bool,
    pub owner: bool,
    #[serde(default)]
    pub blocked: Option<String>,
    #[serde(default)]
    pub repo: Option<HistoryRepo>,
    #[serde(default)]
    pub status: Option<HistoryStatus>,
    #[serde(default)]
    pub branches: Vec<HistoryBranch>,
    #[serde(default)]
    pub git_refs: Vec<HistoryGitRef>,
    #[serde(default)]
    pub changesets: Vec<HistoryChangeset>,
    #[serde(default)]
    pub next_before: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDetail {
    pub changeset: HistoryChangeset,
    #[serde(default)]
    pub changes: Vec<HistoryChange>,
}

// ---- Git API shapes -----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRef {
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitClone {
    pub alias: String,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepository {
    pub id: String,
    pub name: String,
    pub path: String,
    pub parent_id: String,
    pub created_at: f64,
    pub revision: u64,
    pub dirty: i64,
    #[serde(default)]
    pub agent_branches: usize,
    #[serde(default)]
    pub size: f64,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub commits: usize,
    #[serde(default)]
    pub branches: usize,
    #[serde(default)]
    pub tags: usize,
    pub updated_at: f64,
    #[serde(default)]
    pub weeks: Vec<usize>,
    pub clone: GitClone,
    pub role: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub open_issues: usize,
    #[serde(default)]
    pub open_pulls: usize,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommit {
    pub sha: String,
    #[serde(default)]
    pub parents: Vec<String>,
    pub author: String,
    pub time: f64,
    pub subject: String,
    pub source: String,
    #[serde(default)]
    pub refs: Vec<GitRef>,
    #[serde(default)]
    pub added: usize,
    #[serde(default)]
    pub removed: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileChange {
    pub path: String,
    #[serde(default)]
    pub previous_path: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub added: usize,
    #[serde(default)]
    pub removed: usize,
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub patch: Option<String>,
    #[serde(default)]
    pub truncated: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitRestore {
    pub changeset_id: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitDetail {
    pub sha: String,
    #[serde(default)]
    pub parents: Vec<String>,
    pub author: String,
    pub time: f64,
    pub subject: String,
    pub source: String,
    #[serde(default)]
    pub refs: Vec<GitRef>,
    #[serde(default)]
    pub added: usize,
    #[serde(default)]
    pub removed: usize,
    #[serde(default)]
    pub body: String,
    pub committer: String,
    pub committed_at: f64,
    #[serde(default)]
    pub files: Vec<GitFileChange>,
    #[serde(default)]
    pub restore: Option<GitCommitRestore>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitMergeResult {
    pub sha: String,
    pub fast_forward: bool,
    pub deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitPage {
    #[serde(default)]
    pub commits: Vec<GitCommit>,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub next: Option<usize>,
    #[serde(default)]
    pub authors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitTreeEntryLast {
    pub sha: String,
    pub subject: String,
    pub time: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitTreeEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub size: f64,
    #[serde(default)]
    pub last: Option<GitTreeEntryLast>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitTreeReadme {
    pub path: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitTree {
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub sha: String,
    pub path: String,
    #[serde(default)]
    pub entries: Vec<GitTreeEntry>,
    #[serde(default)]
    pub readme: Option<GitTreeReadme>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranch {
    pub name: String,
    pub sha: String,
    pub subject: String,
    pub author: String,
    pub time: f64,
    #[serde(default)]
    pub ahead: usize,
    #[serde(default)]
    pub behind: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitAgentBranch {
    pub name: String,
    pub thread_id: String,
    pub message: String,
    pub author: String,
    pub modified_at: f64,
    #[serde(default)]
    pub files: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranches {
    #[serde(default)]
    pub main: Option<GitBranch>,
    #[serde(default)]
    pub branches: Vec<GitBranch>,
    #[serde(default)]
    pub agent: Vec<GitAgentBranch>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitTag {
    pub name: String,
    pub sha: String,
    pub target: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub author: String,
    pub time: f64,
    #[serde(default)]
    pub annotated: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCompare {
    pub base: String,
    pub head: String,
    pub base_sha: String,
    pub head_sha: String,
    #[serde(default)]
    pub ahead: Vec<GitCommit>,
    #[serde(default)]
    pub behind: usize,
    #[serde(default)]
    pub fast_forward: bool,
    #[serde(default)]
    pub mergeable: bool,
    #[serde(default)]
    pub files: Vec<GitFileChange>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitEvent {
    pub repo_id: String,
    pub repo_name: String,
    pub sha: String,
    pub subject: String,
    pub author: String,
    pub time: f64,
    pub source: String,
    #[serde(default)]
    pub refs: Vec<GitRef>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActivitySources {
    #[serde(default)]
    pub git: usize,
    #[serde(default)]
    pub dar: usize,
    #[serde(default)]
    pub agent: usize,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitActivity {
    #[serde(default)]
    pub times: Vec<f64>,
    #[serde(default)]
    pub events: Vec<GitEvent>,
    #[serde(default)]
    pub sources: GitActivitySources,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLabel {
    pub id: String,
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIssueSummary {
    pub number: u32,
    pub kind: String,
    pub title: String,
    pub state: String,
    pub author: String,
    pub created_at: f64,
    pub updated_at: f64,
    #[serde(default)]
    pub closed_at: Option<f64>,
    #[serde(default)]
    pub comments: usize,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub head: Option<String>,
    #[serde(default)]
    pub agent: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIssueList {
    #[serde(default)]
    pub items: Vec<GitIssueSummary>,
    #[serde(default)]
    pub open: usize,
    #[serde(default)]
    pub closed: usize,
    #[serde(default)]
    pub labels: Vec<GitLabel>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GitTimelineItem {
    #[serde(rename_all = "camelCase")]
    Comment {
        id: String,
        author: String,
        body: String,
        created_at: f64,
        #[serde(default)]
        edited_at: Option<f64>,
        #[serde(default)]
        can_edit: bool,
        #[serde(default)]
        can_delete: bool,
    },
    #[serde(rename_all = "camelCase")]
    Event {
        id: String,
        event: String,
        author: String,
        created_at: f64,
        #[serde(default)]
        number: Option<u32>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        from: Option<String>,
        #[serde(default)]
        to: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        sha: Option<String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIssuePermissions {
    #[serde(default)]
    pub edit: bool,
    #[serde(default)]
    pub close: bool,
    #[serde(default)]
    pub label: bool,
    #[serde(default)]
    pub merge: bool,
    #[serde(default)]
    pub comment: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIssue {
    pub number: u32,
    pub kind: String,
    pub title: String,
    pub state: String,
    pub author: String,
    pub created_at: f64,
    pub updated_at: f64,
    #[serde(default)]
    pub closed_at: Option<f64>,
    #[serde(default)]
    pub comments: usize,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub head: Option<String>,
    #[serde(default)]
    pub agent: Option<bool>,
    #[serde(default)]
    pub body: String,
    pub revision: u64,
    #[serde(default)]
    pub closed_by: Option<String>,
    #[serde(default)]
    pub timeline: Vec<GitTimelineItem>,
    #[serde(default)]
    pub base_sha: Option<String>,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub merged_sha: Option<String>,
    #[serde(default)]
    pub permissions: GitIssuePermissions,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPullAgent {
    #[serde(default)]
    pub changes: Vec<HistoryChange>,
    pub revision: u64,
    pub thread_id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPullChanges {
    #[serde(default)]
    pub compare: Option<GitCompare>,
    #[serde(default)]
    pub agent: Option<GitPullAgent>,
    #[serde(default)]
    pub note: Option<String>,
}

// ---- Shared async & focus helpers ---------------------------------------------------------------

pub fn copy(text: String, notify: Callback<String>, what: &'static str) {
    spawn_local(async move {
        let promise = window().navigator().clipboard().write_text(&text);
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(_) => {
                notify.try_run(format!("Copied {what}"));
            }
            Err(_) => {
                notify.try_run("Select the text and copy it".to_owned());
            }
        }
    });
}

pub struct LoadState<T: Clone + Send + Sync + 'static> {
    pub value: RwSignal<Option<T>>,
    pub error: RwSignal<String>,
    pub retry: Callback<()>,
    sequence: RwSignal<u64>,
}

impl<T: Clone + Send + Sync + 'static> Clone for LoadState<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Clone + Send + Sync + 'static> Copy for LoadState<T> {}

impl<T: Clone + Send + Sync + 'static> LoadState<T> {
    /// Sets a value fetched outside the loader, discarding any older request still in flight.
    pub fn set(&self, next: T) {
        let _ = self.sequence.try_update(|n| *n += 1);
        let _ = self.value.try_set(Some(next));
    }
}

pub fn use_load<T>(url: Signal<Option<String>>, trigger: Signal<u64>) -> LoadState<T>
where
    T: DeserializeOwned + Clone + Send + Sync + 'static,
{
    let value: RwSignal<Option<T>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let attempt = RwSignal::new(0u64);
    let sequence = RwSignal::new(0u64);
    Effect::new(move |_| {
        let target = url.get();
        let _ = trigger.get();
        let _ = attempt.get();
        let Some(target) = target else { return };
        let id = sequence.get_untracked() + 1;
        sequence.set(id);
        error.set(String::new());
        let abort = web_sys::AbortController::new().ok();
        let signal = abort.as_ref().map(|a| a.signal());
        if let Some(abort) = abort {
            let abort = send_wrapper::SendWrapper::new(abort);
            on_cleanup(move || abort.abort());
        }
        spawn_local(async move {
            let check = signal.clone();
            let result = api::request::<T>(
                &target,
                Req {
                    signal,
                    ..Default::default()
                },
            )
            .await;
            if sequence.try_get_untracked() != Some(id)
                || check.as_ref().is_some_and(|s| s.aborted())
            {
                return;
            }
            match result {
                Ok(v) => {
                    let _ = value.try_set(Some(v));
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                }
            }
        });
    });
    let retry = Callback::new(move |_| attempt.update(|n| *n += 1));
    LoadState {
        value,
        error,
        retry,
        sequence,
    }
}

pub fn focus_sheet(root: NodeRef<html::Div>) {
    Effect::new(move |_| {
        if let Some(el) = root.get() {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = el.focus_with_options(&options);
        }
    });
}

pub fn focus_aside(root: NodeRef<html::Aside>) {
    Effect::new(move |_| {
        if let Some(el) = root.get() {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = el.focus_with_options(&options);
        }
    });
}

pub fn sheet_keydown(event: web_sys::KeyboardEvent, on_close: Callback<()>) {
    if event.key() == "Escape" {
        let inside_popup = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| {
                el.closest("[role=menu],[role=listbox],[role=dialog]")
                    .ok()
                    .flatten()
            })
            .is_some();
        if !inside_popup {
            event.prevent_default();
            event.stop_propagation();
            {
                on_close.try_run(());
            };
        }
    }
}

/// Schedules a one-shot timeout cleared automatically if the owning component unmounts.
pub fn schedule_notice_clear(notice: RwSignal<Option<Notice>>, id: u64) {
    let handle = set_timeout_with_handle(
        move || {
            if notice
                .try_get_untracked()
                .flatten()
                .is_some_and(|n| n.id == id)
            {
                let _ = notice.try_set(None);
            }
        },
        Duration::from_millis(5000),
    );
    if let Ok(handle) = handle {
        on_cleanup(move || handle.clear());
    }
}

/// Keeps `session` unused warnings away when passed purely for CSRF tokens.
pub fn csrf_of(session: &BrowserSession) -> String {
    session.csrf_token.clone()
}

pub fn blocked_shortcut_target(event: &web_sys::KeyboardEvent) -> bool {
    event
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .and_then(|el| {
            el.closest("input,textarea,select,[contenteditable],[role=dialog],[role=menu],.editing-pane,.settings-sheet")
                .ok()
                .flatten()
        })
        .is_some()
}
