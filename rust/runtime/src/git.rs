//! Git over HTTPS for tracked folders (`runtime/files/git.ts`, docs/spacetimedb/folder-history.md#git).
//! The database keeps the history; each tracked folder has a bare repository under `<root>/git` as
//! a cache, rebuilt from the database when missing. Clones and fetches run `git upload-pack` on it.
//! Pushes are received into a quarantine, read with Git, and applied by the database in one
//! transaction before Git reports success.
//!
//! Objects are written with `git hash-object` and request bodies inflated with `gzip`, so the
//! runtime needs no SHA-1 or zlib code of its own.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout};
use tokio_util::io::ReaderStream;

use crate::blobs::Blobs;
use crate::db::Database;
use crate::error::{AppError, Result};

pub const ZERO: &str = "0000000000000000000000000000000000000000";
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
pub const MAIN_REF: &str = "refs/heads/main";
const READ_OPERATIONS: [&str; 5] = ["state", "export", "repositories", "resolve", "changesetOf"];
const PACK_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const OBJECT_BYTES: usize = 256 * 1024;
const COMMITS: usize = 5000;
const CHANGES: usize = 50_000;
const MESSAGE_UNITS: usize = 1024;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

pub type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

// ---- Git processes --------------------------------------------------------------------------

pub fn path_str(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

/// Git with no user or system configuration, hooks or network.
pub fn git_command(home: &Path, args: &[&str], env: &[(&str, &str)]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["-c", "core.hooksPath=/dev/null", "-c", "protocol.allow=never"])
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in env {
        command.env(key, value);
    }
    command
}

pub struct Out {
    pub stdout: Vec<u8>,
    pub code: i32,
}

impl Out {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_owned()
    }
}

fn unavailable() -> AppError {
    AppError::with("gitUnavailable", 503, "Git is not available on this server.")
}
fn failed() -> AppError {
    AppError::with("gitFailed", 502, "Git could not complete the request.")
}

/// Runs Git to completion with `input`; Git's error text stays out of responses.
pub async fn git_full(home: &Path, args: &[&str], env: &[(&str, &str)], input: &[u8], allow_failure: bool, timeout: Duration) -> Result<Out> {
    let mut child = git_command(home, args, env).spawn().map_err(|_| unavailable())?;
    let mut stdin = child.stdin.take().expect("stdin");
    let data = input.to_vec();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&data).await;
        drop(stdin);
    });
    let output = tokio::time::timeout(timeout, child.wait_with_output()).await.map_err(|_| failed())?.map_err(|_| unavailable())?;
    let _ = writer.await;
    let code = output.status.code().unwrap_or(-1);
    if code != 0 && !allow_failure {
        return Err(failed());
    }
    Ok(Out { stdout: output.stdout, code })
}

pub async fn git(home: &Path, args: &[&str]) -> Result<Out> {
    git_full(home, args, &[], b"", false, Duration::from_secs(300)).await
}
pub async fn git_in(home: &Path, args: &[&str], input: &[u8]) -> Result<Out> {
    git_full(home, args, &[], input, false, Duration::from_secs(300)).await
}
pub async fn git_may_fail(home: &Path, args: &[&str]) -> Result<Out> {
    git_full(home, args, &[], b"", true, Duration::from_secs(300)).await
}

pub fn lines(value: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(value).split('\n').filter(|l| !l.is_empty()).map(str::to_owned).collect()
}

fn hex_bytes(sha: &str) -> Vec<u8> {
    (0..sha.len() / 2).map(|i| u8::from_str_radix(&sha[2 * i..2 * i + 2], 16).unwrap_or(0)).collect()
}

pub fn is_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ---- Objects --------------------------------------------------------------------------------

/// Writes loose objects; existing ones (loose or packed) are left alone.
struct ObjectWriter {
    dir: PathBuf,
}

impl ObjectWriter {
    fn file(&self, sha: &str) -> PathBuf {
        self.dir.join("objects").join(&sha[..2]).join(&sha[2..])
    }
    fn exists(&self, sha: &str) -> bool {
        is_sha(sha) && self.file(sha).exists()
    }
    async fn write(&self, kind: &str, content: &[u8]) -> Result<String> {
        let out = git_in(&self.dir, &["--git-dir", path_str(&self.dir), "hash-object", "-w", "--literally", "-t", kind, "--stdin"], content).await?;
        Ok(out.text())
    }
    /// A stored blob as a Git blob, returning its ID.
    async fn write_blob(&self, file: &Path, size: f64) -> Result<String> {
        let length = tokio::fs::metadata(file).await.map_err(|_| AppError::new("contentUnavailable", 503))?.len();
        if length as f64 != size {
            return Err(AppError::new("contentUnavailable", 503));
        }
        Ok(git(&self.dir, &["--git-dir", path_str(&self.dir), "hash-object", "-w", "--no-filters", "--", path_str(file)]).await?.text())
    }
}

/// A tree being built; unchanged subtrees keep their IDs.
#[derive(Clone)]
enum Node {
    Blob { mode: String, sha: String },
    Tree(Arc<TreeNode>),
}

#[derive(Default)]
struct TreeNode {
    entries: BTreeMap<String, Node>,
    sha: OnceLock<String>,
}

fn empty_node() -> Arc<TreeNode> {
    let node = TreeNode::default();
    let _ = node.sha.set(EMPTY_TREE.into());
    Arc::new(node)
}

fn set_path(node: &TreeNode, parts: &[&str], leaf: Option<Node>) -> Arc<TreeNode> {
    let mut entries = node.entries.clone();
    let (name, rest) = (parts[0], &parts[1..]);
    if rest.is_empty() {
        match leaf {
            Some(leaf) => {
                entries.insert(name.into(), leaf);
            }
            None => {
                entries.remove(name);
            }
        }
    } else {
        let child = match entries.get(name) {
            Some(Node::Tree(t)) => t.clone(),
            _ => empty_node(),
        };
        let next = set_path(&child, rest, leaf);
        if next.entries.is_empty() {
            entries.remove(name);
        } else {
            entries.insert(name.into(), Node::Tree(next));
        }
    }
    Arc::new(TreeNode { entries, sha: OnceLock::new() })
}

/// Git's tree order: names compared as bytes, a folder as if its name ended in "/".
fn hash_tree<'a>(node: &'a TreeNode, objects: &'a ObjectWriter) -> Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
    Box::pin(async move {
        if let Some(sha) = node.sha.get() {
            return Ok(sha.clone());
        }
        let mut items: Vec<(Vec<u8>, String, String, String)> = Vec::new();
        for (name, child) in &node.entries {
            let (key, mode, sha) = match child {
                Node::Tree(t) => (format!("{name}/"), "40000".to_owned(), hash_tree(t, objects).await?),
                Node::Blob { mode, sha } => (name.clone(), if mode.is_empty() { "100644".into() } else { mode.clone() }, sha.clone()),
            };
            items.push((key.into_bytes(), name.clone(), mode, sha));
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        let mut content = Vec::new();
        for (_, name, mode, sha) in items {
            content.extend_from_slice(format!("{mode} {name}\0").as_bytes());
            content.extend_from_slice(&hex_bytes(&sha));
        }
        let sha = objects.write("tree", &content).await?;
        let _ = node.sha.set(sha.clone());
        Ok(sha)
    })
}

/// A commit's tree, read back from the repository.
async fn load_tree(dir: &Path, commit: &str) -> Result<Arc<TreeNode>> {
    let out = git(dir, &["--git-dir", path_str(dir), "ls-tree", "-r", "-t", "-z", "--full-tree", commit]).await?;
    // Built bottom-up: deeper paths first, so each tree is complete when its parent takes it.
    let mut records: Vec<(String, String, String, String)> = Vec::new();
    for record in String::from_utf8_lossy(&out.stdout).split('\0').filter(|r| !r.is_empty()) {
        let Some((meta, file)) = record.split_once('\t') else { continue };
        let parts: Vec<&str> = meta.split(' ').collect();
        if parts.len() < 3 {
            continue;
        }
        records.push((parts[0].into(), parts[1].into(), parts[2].into(), file.into()));
    }
    let mut trees: HashMap<String, TreeNode> = HashMap::new();
    trees.insert(String::new(), TreeNode::default());
    for (_, kind, sha, file) in &records {
        if kind == "tree" {
            let node = TreeNode::default();
            let _ = node.sha.set(sha.clone());
            trees.insert(file.clone(), node);
        }
    }
    for (mode, kind, sha, file) in &records {
        if kind == "tree" {
            continue;
        }
        let (parent, name) = file.rsplit_once('/').unwrap_or(("", file));
        if let Some(t) = trees.get_mut(parent) {
            t.entries.insert(name.into(), Node::Blob { mode: if mode == "100644" { String::new() } else { mode.clone() }, sha: sha.clone() });
        }
    }
    let mut folders: Vec<String> = records.iter().filter(|r| r.1 == "tree").map(|r| r.3.clone()).collect();
    folders.sort_by_key(|f| std::cmp::Reverse(f.matches('/').count()));
    for folder in folders {
        let node = trees.remove(&folder).expect("tree");
        let (parent, name) = folder.rsplit_once('/').unwrap_or(("", &folder));
        if let Some(t) = trees.get_mut(parent) {
            t.entries.insert(name.into(), Node::Tree(Arc::new(node)));
        }
    }
    Ok(Arc::new(trees.remove("").expect("root")))
}

// ---- History from the database --------------------------------------------------------------

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ExportChange {
    path: String,
    #[serde(default)]
    previous_path: String,
    kind: String,
    #[serde(default)]
    mode: String,
    blob_id: Option<String>,
    size: Option<f64>,
    sha: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Author {
    #[serde(default)]
    name: String,
    #[serde(default)]
    username: String,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ExportCommit {
    id: String,
    #[serde(default)]
    parent_id: String,
    message: String,
    created_at: f64,
    author: Author,
    sha: Option<String>,
    raw: Option<String>,
    changes: Vec<ExportChange>,
}

#[derive(Deserialize, Clone)]
pub struct RefRow {
    pub name: String,
    pub sha: String,
}

#[derive(Deserialize, Clone)]
pub struct GitState {
    pub revision: f64,
    pub generation: f64,
    pub main: Option<String>,
    base: Option<String>,
    pending: Vec<ExportCommit>,
    pub refs: Vec<RefRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitExport {
    #[serde(default)]
    head_id: String,
    commits: Vec<ExportCommit>,
    tags: Vec<TagRow>,
}

#[derive(Deserialize)]
struct TagRow {
    raw: String,
}

#[derive(Deserialize, serde::Serialize)]
struct LocalState {
    generation: f64,
    revision: f64,
}

fn clean(value: &str) -> String {
    value.chars().filter(|c| !"<>\n\r\0".contains(*c)).collect::<String>().trim().to_owned()
}

/// The Git commit for a Dar commit: same tree, author and time every time it is made.
fn native_commit(tree: &str, parent: Option<&str>, commit: &ExportCommit) -> Vec<u8> {
    let name = Some(clean(&commit.author.name)).filter(|n| !n.is_empty()).or_else(|| Some(clean(&commit.author.username)).filter(|n| !n.is_empty())).unwrap_or_else(|| "Dar".into());
    let user: String = commit.author.username.chars().filter(|c| !"<>".contains(*c) && !c.is_whitespace()).collect();
    let who = format!("{name} <{}@dar.invalid> {} +0000", if user.is_empty() { "dar" } else { &user }, (commit.created_at / 1000.0).floor() as i64);
    let parent = parent.map(|p| format!("parent {p}\n")).unwrap_or_default();
    format!("tree {tree}\n{parent}author {who}\ncommitter {who}\n\n{}\n", commit.message).into_bytes()
}

fn header_of(raw: &[u8]) -> Vec<String> {
    let end = raw.windows(2).position(|w| w == b"\n\n").unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).split('\n').map(str::to_owned).collect()
}

fn mismatch() -> AppError {
    AppError::with("historyMismatch", 500, "This folder's history does not match its Git objects.")
}
fn history_unavailable() -> AppError {
    AppError::with("historyUnavailable", 503, "This folder's history could not be read.")
}

struct Materialized {
    pins: Vec<Value>,
    blobs: Vec<Value>,
}

/// Per-folder repositories under `<root>/git`, one operation at a time per folder.
pub struct Repositories {
    db: Database,
    base: PathBuf,
    pub blobs: Blobs,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Folders whose repository may have new objects since the last maintenance.
    touched: Mutex<HashSet<String>>,
}

pub type Guard = tokio::sync::OwnedMutexGuard<()>;

impl Repositories {
    pub fn new(db: Database, root: &Path, blobs: Blobs) -> Self {
        Repositories { db, base: root.join("git"), blobs, locks: Mutex::new(HashMap::new()), touched: Mutex::new(HashSet::new()) }
    }

    pub fn dir_of(&self, folder_id: &str) -> PathBuf {
        self.base.join(format!("{folder_id}.git"))
    }

    /// The folder's lock; work on its repository holds it.
    pub async fn lock(&self, folder_id: &str) -> Guard {
        self.touched.lock().expect("touched").insert(folder_id.to_owned());
        let lock = {
            let mut locks = self.locks.lock().expect("locks");
            locks.retain(|_, l| Arc::strong_count(l) > 1);
            locks.entry(folder_id.to_owned()).or_default().clone()
        };
        lock.lock_owned().await
    }

    pub async fn call(&self, session_id: &str, operation: &str, document: Value) -> Result<Value> {
        self.db.operation("service_git", operation, &READ_OPERATIONS, json!([session_id, operation, document.to_string()])).await
    }

    async fn prepare(&self, dir: &Path, generation: f64) -> Result<LocalState> {
        if let Ok(text) = tokio::fs::read(dir.join("dar-state.json")).await
            && let Ok(state) = serde_json::from_slice::<LocalState>(&text)
            && state.generation == generation
        {
            return Ok(state);
        }
        let _ = tokio::fs::remove_dir_all(dir).await;
        create_private_dir(&self.base).await?;
        git(&self.base, &["init", "--bare", "-q", "-b", "main", path_str(dir)]).await?;
        let config = dir.join("config");
        for (key, value) in [("uploadpack.allowFilter", "true"), ("uploadpack.allowReachableSHA1InWant", "true"), ("gc.autoDetach", "false"), ("core.logAllRefUpdates", "false"), ("receive.denyDeletes", "true")] {
            git(&self.base, &["config", "--file", path_str(&config), key, value]).await?;
        }
        let state = LocalState { generation, revision: -1.0 };
        save_state(dir, &state).await?;
        Ok(state)
    }

    async fn missing(&self, dir: &Path, shas: &[String]) -> Result<Vec<String>> {
        if shas.is_empty() {
            return Ok(Vec::new());
        }
        let out = git_in(dir, &["--git-dir", path_str(dir), "cat-file", "--batch-check"], format!("{}\n", shas.join("\n")).as_bytes()).await?;
        Ok(lines(&out.stdout).into_iter().filter(|l| l.ends_with(" missing")).map(|l| l.split(' ').next().unwrap_or_default().to_owned()).collect())
    }

    /// Writes Git objects for commits (base first) from their bases' trees, and the Git commits
    /// Dar's own commits get. Returns the pins to record.
    async fn materialize(&self, dir: &Path, commits: &mut [ExportCommit], trees: &mut HashMap<String, Arc<TreeNode>>, shas: &mut HashMap<String, String>) -> Result<Materialized> {
        let objects = ObjectWriter { dir: dir.to_owned() };
        let (mut pins, mut blob_pins, mut hashed) = (Vec::new(), BTreeMap::<String, Value>::new(), HashMap::<String, String>::new());
        let mut children: HashMap<String, usize> = HashMap::new();
        for commit in commits.iter() {
            *children.entry(commit.parent_id.clone()).or_default() += 1;
        }
        for commit in commits.iter_mut() {
            let mut tree = if commit.parent_id.is_empty() { empty_node() } else { trees.get(&commit.parent_id).cloned().ok_or_else(history_unavailable)? };
            for change in &commit.changes {
                if change.kind == "deleted" || change.kind == "renamed" {
                    let from = if change.kind == "renamed" { &change.previous_path } else { &change.path };
                    tree = set_path(&tree, &from.split('/').collect::<Vec<_>>(), None);
                }
            }
            for change in &commit.changes {
                if change.kind == "deleted" {
                    continue;
                }
                let blob_id = change.blob_id.clone().unwrap_or_default();
                let mut sha = change.sha.clone().filter(|s| !s.is_empty()).or_else(|| hashed.get(&blob_id).cloned());
                if sha.as_ref().is_none_or(|s| !objects.exists(s)) {
                    let file = self.blobs.path(&blob_id)?;
                    let written = objects.write_blob(&file, change.size.unwrap_or(0.0)).await?;
                    if sha.as_ref().is_some_and(|s| *s != written) {
                        return Err(mismatch());
                    }
                    if change.sha.as_ref().is_none_or(|s| s.is_empty()) {
                        blob_pins.insert(written.clone(), json!({ "sha": written, "blobId": blob_id, "size": change.size.unwrap_or(0.0) }));
                    }
                    sha = Some(written);
                }
                let sha = sha.expect("sha");
                hashed.insert(blob_id, sha.clone());
                tree = set_path(&tree, &change.path.split('/').collect::<Vec<_>>(), Some(Node::Blob { mode: change.mode.clone(), sha }));
            }
            let tree_sha = hash_tree(&tree, &objects).await?;
            if let Some(raw) = commit.raw.as_ref().filter(|r| !r.is_empty()) {
                let raw = base64::engine::general_purpose::STANDARD.decode(raw).map_err(|_| mismatch())?;
                if header_of(&raw).first().map(String::as_str) != Some(format!("tree {tree_sha}").as_str()) {
                    return Err(mismatch());
                }
                if Some(objects.write("commit", &raw).await?) != commit.sha {
                    return Err(mismatch());
                }
            } else {
                let parent = if commit.parent_id.is_empty() { None } else { Some(shas.get(&commit.parent_id).cloned().ok_or_else(history_unavailable)?) };
                let raw = native_commit(&tree_sha, parent.as_deref(), commit);
                let sha = objects.write("commit", &raw).await?;
                commit.sha = Some(sha.clone());
                let author = Some(clean(&commit.author.name)).filter(|n| !n.is_empty()).unwrap_or_else(|| commit.author.username.clone());
                pins.push(json!({ "id": commit.id, "sha": sha, "raw": base64::engine::general_purpose::STANDARD.encode(&raw), "parents": parent.map(|p| vec![p]).unwrap_or_default(), "author": author }));
            }
            shas.insert(commit.id.clone(), commit.sha.clone().unwrap_or_default());
            trees.insert(commit.id.clone(), tree);
            // A tree is kept only while commits based on it are still to come.
            let left = children.get(&commit.parent_id).copied().unwrap_or(1).saturating_sub(1);
            children.insert(commit.parent_id.clone(), left);
            if left == 0 {
                trees.remove(&commit.parent_id);
            }
            if children.get(&commit.id).copied().unwrap_or(0) == 0 {
                trees.remove(&commit.id);
            }
        }
        Ok(Materialized { pins, blobs: blob_pins.into_values().collect() })
    }

    async fn pin(&self, session_id: &str, folder_id: &str, result: Materialized) -> Result<()> {
        let count = result.pins.len().max(result.blobs.len());
        let mut i = 0;
        while i < count {
            let commits: Vec<Value> = result.pins.iter().skip(i).take(1000).cloned().collect();
            let blobs: Vec<Value> = result.blobs.iter().skip(i).take(1000).cloned().collect();
            let answer = self.call(session_id, "pin", json!({ "folderId": folder_id, "commits": commits, "blobs": blobs })).await?;
            if answer["conflict"] == json!(true) {
                return Err(AppError::with("conflict", 409, "The folder's history changed; try again."));
            }
            i += 1000;
        }
        Ok(())
    }

    async fn set_refs(&self, dir: &Path, wanted: &BTreeMap<String, String>) -> Result<()> {
        let out = git(dir, &["--git-dir", path_str(dir), "for-each-ref", "--format=%(objectname) %(refname)"]).await?;
        let have: HashMap<String, String> = lines(&out.stdout).into_iter().filter_map(|l| l.split_once(' ').map(|(s, n)| (n.to_owned(), s.to_owned()))).collect();
        let mut input = String::new();
        for (name, sha) in wanted {
            if have.get(name) != Some(sha) {
                input += &format!("update {name} {sha}\n");
            }
        }
        for name in have.keys() {
            if !wanted.contains_key(name) {
                input += &format!("delete {name}\n");
            }
        }
        if !input.is_empty() {
            git_in(dir, &["--git-dir", path_str(dir), "update-ref", "--stdin"], input.as_bytes()).await?;
        }
        Ok(())
    }

    /// Brings a folder's repository up to date with its history. The caller holds the lock.
    pub async fn sync_locked(&self, session_id: &str, folder_id: &str) -> Result<(PathBuf, GitState)> {
        let state: GitState = serde_json::from_value(self.call(session_id, "state", json!({ "folderId": folder_id })).await?).map_err(|_| history_unavailable())?;
        let dir = self.dir_of(folder_id);
        let local = self.prepare(&dir, state.generation).await?;
        if local.revision == state.revision && state.pending.is_empty() {
            return Ok((dir, state));
        }
        let mut shas: HashMap<String, String> = HashMap::new();
        let mut main = state.main.clone();
        let mut needed: Vec<String> = Vec::new();
        for sha in state.main.iter().chain(state.base.iter()).chain(state.refs.iter().map(|r| &r.sha)) {
            if !sha.is_empty() && !needed.contains(sha) {
                needed.push(sha.clone());
            }
        }
        if !self.missing(&dir, &needed).await?.is_empty() {
            // The cache is new or incomplete: rebuild every commit from the database.
            let all: GitExport = serde_json::from_value(self.call(session_id, "export", json!({ "folderId": folder_id })).await?).map_err(|_| history_unavailable())?;
            let by_id: HashMap<String, ExportCommit> = all.commits.iter().map(|c| (c.id.clone(), c.clone())).collect();
            let mut ordered: Vec<ExportCommit> = Vec::new();
            let mut done: HashSet<String> = HashSet::from([String::new()]);
            for commit in &all.commits {
                let mut stack = vec![commit.id.clone()];
                while let Some(top) = stack.last().cloned() {
                    if done.contains(&top) {
                        stack.pop();
                        continue;
                    }
                    let item = &by_id[&top];
                    let parent = by_id.get(&item.parent_id);
                    if !item.parent_id.is_empty() && parent.is_none() {
                        return Err(history_unavailable());
                    }
                    if let Some(parent) = parent
                        && !done.contains(&parent.id)
                    {
                        stack.push(parent.id.clone());
                        continue;
                    }
                    done.insert(top.clone());
                    ordered.push(item.clone());
                    stack.pop();
                }
            }
            for commit in &ordered {
                if let Some(sha) = commit.sha.as_ref().filter(|s| !s.is_empty()) {
                    shas.insert(commit.id.clone(), sha.clone());
                }
            }
            let made = self.materialize(&dir, &mut ordered, &mut HashMap::new(), &mut shas).await?;
            self.pin(session_id, folder_id, made).await?;
            let objects = ObjectWriter { dir: dir.clone() };
            for tag in &all.tags {
                objects.write("tag", &base64::engine::general_purpose::STANDARD.decode(&tag.raw).map_err(|_| mismatch())?).await?;
            }
            main = if all.head_id.is_empty() { None } else { shas.get(&all.head_id).cloned() };
        } else if !state.pending.is_empty() {
            let mut pending = state.pending.clone();
            let mut trees: HashMap<String, Arc<TreeNode>> = HashMap::new();
            let first = pending[0].parent_id.clone();
            if !first.is_empty() {
                let base = state.base.clone().unwrap_or_default();
                trees.insert(first.clone(), load_tree(&dir, &base).await?);
                shas.insert(first, base);
            }
            let made = self.materialize(&dir, &mut pending, &mut trees, &mut shas).await?;
            self.pin(session_id, folder_id, made).await?;
            main = shas.get(&pending.last().expect("pending").id).cloned();
        }
        let mut wanted: BTreeMap<String, String> = state.refs.iter().map(|r| (r.name.clone(), r.sha.clone())).collect();
        if let Some(main) = &main {
            wanted.insert(MAIN_REF.into(), main.clone());
        }
        self.set_refs(&dir, &wanted).await?;
        save_state(&dir, &LocalState { generation: state.generation, revision: state.revision }).await?;
        Ok((dir, GitState { main, ..state }))
    }

    pub async fn sync(&self, session_id: &str, folder_id: &str) -> Result<(PathBuf, GitState)> {
        let _guard = self.lock(folder_id).await;
        self.sync_locked(session_id, folder_id).await
    }

    /// `git gc --auto` with low thresholds; unreachable objects go at once (the repository is a cache).
    async fn compact(&self, dir: &Path) {
        if !dir.exists() {
            return;
        }
        let args = [
            "--git-dir", path_str(dir), "-c", "gc.auto=250", "-c", "gc.autoPackLimit=8", "-c", "gc.autoDetach=false", "-c", "gc.pruneExpire=now", "-c", "repack.writeBitmaps=true", "-c", "gc.writeCommitGraph=true", "gc", "--auto", "--quiet",
        ];
        let _ = git_full(dir, &args, &[], b"", true, Duration::from_secs(1800)).await;
    }

    /// Removes repositories of folders that no longer keep history, and packs the ones used since last time.
    pub async fn maintain(&self) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.base).await else { return };
        let Ok(rows) = self.db.read("service_git_repos", json!([])).await else { return };
        let repos: HashSet<String> = rows.as_array().map(|r| r.iter().filter_map(|row| row["id"].as_str().map(|id| format!("{id}.git"))).collect()).unwrap_or_default();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            let folder = name.strip_suffix(".git").filter(|f| f.len() == 36 && f.bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-')));
            if let Some(folder) = folder
                && !repos.contains(&name)
            {
                let _guard = self.lock(folder).await;
                let _ = tokio::fs::remove_dir_all(self.base.join(&name)).await;
            }
        }
        let folders: Vec<String> = self.touched.lock().expect("touched").drain().collect();
        for folder in &folders {
            if repos.contains(&format!("{folder}.git")) {
                let _guard = self.lock(folder).await;
                self.compact(&self.dir_of(folder)).await;
            }
        }
        // lock() marks each folder it runs for; packing is not a change.
        let mut touched = self.touched.lock().expect("touched");
        for folder in &folders {
            touched.remove(folder);
        }
    }

    /// main and every branch and tag as a Git bundle, for the History tab's download.
    pub async fn bundle(&self, session_id: &str, folder_id: &str, name: &str) -> Result<Response> {
        let (dir, state) = self.sync(session_id, folder_id).await?;
        if state.main.is_none() && state.refs.is_empty() {
            return Err(AppError::with("nothingToExport", 409, "Commit something before exporting."));
        }
        let work = self.base.join(format!("bundle-{}", crate::auth::uuid()));
        create_private_dir(&work).await?;
        let file = work.join("history.bundle");
        let made = git(&dir, &["--git-dir", path_str(&dir), "bundle", "create", "-q", path_str(&file), "--all"]).await;
        let opened = match made {
            Ok(_) => tokio::fs::File::open(&file).await.map_err(|_| AppError::internal()),
            Err(e) => Err(e),
        };
        // The open file outlives its name, so the work folder goes now.
        let _ = tokio::fs::remove_dir_all(&work).await;
        let handle = opened?;
        let size = handle.metadata().await.map_err(|_| AppError::internal())?.len();
        let cleaned: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '_' || " .-".contains(*c)).collect::<String>().trim().to_owned();
        let filename = format!("{}.bundle", if cleaned.is_empty() { "history" } else { &cleaned });
        let mut response = Body::from_stream(ReaderStream::new(handle)).into_response();
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
        headers.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{}", encode_component(&filename))).map_err(|_| AppError::internal())?);
        Ok(response)
    }
}

async fn create_private_dir(dir: &Path) -> Result<()> {
    tokio::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).await.map_err(|_| AppError::internal())
}

async fn save_state(dir: &Path, state: &LocalState) -> Result<()> {
    let file = dir.join("dar-state.json");
    tokio::fs::write(&file, serde_json::to_vec(state).expect("state")).await.map_err(|_| AppError::internal())?;
    let _ = tokio::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600)).await;
    Ok(())
}

/// `encodeURIComponent`.
pub fn encode_component(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// `decodeURIComponent`; `None` for malformed escapes or bytes that are not UTF-8.
pub fn decode_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

// ---- Smart HTTP -----------------------------------------------------------------------------

pub fn pkt(payload: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", payload.len() + 4).into_bytes();
    out.extend_from_slice(payload);
    out
}
const FLUSH: &[u8] = b"0000";

/// A request body, inflated when Git compressed it.
pub fn body_stream(headers: &HeaderMap, body: Body) -> Result<ByteStream> {
    let raw: ByteStream = Box::pin(body.into_data_stream().map(|r| r.map_err(io::Error::other)));
    match headers.get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()) {
        None | Some("identity") => Ok(raw),
        Some("gzip" | "x-gzip") => gunzip(raw),
        Some(_) => Err(AppError::new("unsupportedEncoding", 415)),
    }
}

fn gunzip(mut raw: ByteStream) -> Result<ByteStream> {
    let mut child = tokio::process::Command::new("gzip")
        .arg("-dc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| AppError::with("unsupportedEncoding", 415, "This server cannot read compressed requests."))?;
    let mut stdin = child.stdin.take().expect("stdin");
    tokio::spawn(async move {
        while let Some(Ok(chunk)) = raw.next().await {
            if stdin.write_all(&chunk).await.is_err() {
                break;
            }
        }
    });
    let stdout = child.stdout.take().expect("stdout");
    Ok(Box::pin(with_exit(ReaderStream::new(stdout), child)))
}

/// A child's output, ending in an error when the child failed.
fn with_exit<S>(output: S, mut child: Child) -> impl Stream<Item = io::Result<Bytes>> + Send
where
    S: Stream<Item = io::Result<Bytes>> + Send,
{
    output.chain(stream::once(async move {
        match child.wait().await {
            Ok(status) if status.success() => Ok(Bytes::new()),
            _ => Err(io::Error::other("the process failed")),
        }
    }))
}

/// Reads pkt-lines from a request, then hands over the rest (the pack).
struct PacketReader {
    buffer: Vec<u8>,
    done: bool,
    source: ByteStream,
}

impl PacketReader {
    async fn fill(&mut self, size: usize) -> bool {
        while self.buffer.len() < size && !self.done {
            match self.source.next().await {
                Some(Ok(chunk)) => self.buffer.extend_from_slice(&chunk),
                _ => self.done = true,
            }
        }
        self.buffer.len() >= size
    }
    async fn packet(&mut self) -> Result<Option<Vec<u8>>> {
        let early = || AppError::with("invalidRequest", 400, "The Git request ended early.");
        if !self.fill(4).await {
            return Err(early());
        }
        let length = std::str::from_utf8(&self.buffer[..4]).ok().and_then(|s| usize::from_str_radix(s, 16).ok());
        let Some(length) = length.filter(|l| !(1..=3).contains(l) && *l <= 65520) else { return Err(AppError::with("invalidRequest", 400, "The Git request is malformed.")) };
        if length == 0 {
            self.buffer.drain(..4);
            return Ok(None);
        }
        if !self.fill(length).await {
            return Err(early());
        }
        let payload = self.buffer[4..length].to_vec();
        self.buffer.drain(..length);
        Ok(Some(payload))
    }
    fn rest(self) -> ByteStream {
        let first = Bytes::from(self.buffer);
        Box::pin(stream::once(async move { Ok(first) }).chain(self.source))
    }
}

/// Reads `git cat-file --batch` output object by object.
struct BatchReader {
    reader: BufReader<ChildStdout>,
}

struct BatchHeader {
    sha: String,
    kind: String,
    size: usize,
}

impl BatchReader {
    fn broken() -> AppError {
        AppError::with("gitFailed", 502, "Git could not read the push.")
    }
    async fn header(&mut self) -> Result<BatchHeader> {
        let mut line = Vec::new();
        if self.reader.read_until(b'\n', &mut line).await.map_err(|_| Self::broken())? == 0 {
            return Err(Self::broken());
        }
        let text = String::from_utf8_lossy(&line).trim_end().to_owned();
        let parts: Vec<&str> = text.split(' ').collect();
        if parts.get(1) == Some(&"missing") || parts.len() < 3 {
            return Err(AppError::with("invalidRequest", 400, "The push is missing objects."));
        }
        Ok(BatchHeader { sha: parts[0].into(), kind: parts[1].into(), size: parts[2].parse().unwrap_or(0) })
    }
    async fn read(&mut self, size: usize) -> Result<Vec<u8>> {
        let mut data = vec![0u8; size + 1];
        self.reader.read_exact(&mut data).await.map_err(|_| Self::broken())?;
        data.pop();
        Ok(data)
    }
}

fn git_service_error(code: &str, message: &str, status: u16) -> AppError {
    AppError::with(code, status, message)
}

/// Parsed `/dav/git/...` address.
struct Target {
    folder: Option<String>,
    alias: Option<(String, String)>,
    route: &'static str,
}

fn uuid_like(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { matches!(c, b'a'..=b'f' | b'0'..=b'9') })
}

fn target_of(path: &str) -> Result<Target> {
    let rest = path.strip_prefix("/dav/git/").ok_or_else(AppError::not_found)?;
    let (middle, route) = ["/info/refs", "/git-upload-pack", "/git-receive-pack"].iter().find_map(|r| rest.strip_suffix(r).map(|m| (m, *r))).ok_or_else(AppError::not_found)?;
    let id = middle.strip_suffix(".git").unwrap_or(middle);
    if uuid_like(id) && !middle.contains('/') {
        return Ok(Target { folder: Some(id.into()), alias: None, route });
    }
    let (user, folder) = middle.strip_suffix(".git").and_then(|m| m.split_once('/')).ok_or_else(AppError::not_found)?;
    if user.is_empty() || folder.is_empty() {
        return Err(AppError::not_found());
    }
    let user = decode_component(user).ok_or_else(AppError::not_found)?;
    let folder = folder.split('/').map(decode_component).collect::<Option<Vec<_>>>().ok_or_else(AppError::not_found)?.join("/");
    Ok(Target { folder: None, alias: Some((user, folder)), route })
}

/// `/dav/git/<folderId>.git`: Git's smart HTTP protocol, signed in with an app password with Files access.
pub async fn handle_http(runtime: crate::http::Shared, method: &Method, path: &str, query: &HashMap<String, String>, headers: &HeaderMap, body: Body) -> Result<Response> {
    let target = target_of(path)?;
    let origin = &runtime.config.origin;
    if let Some(o) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        && o != origin
    {
        return Err(AppError::new("invalidOrigin", 403));
    }
    let service = if target.route == "/info/refs" { query.get("service").cloned().unwrap_or_default() } else { target.route[1..].to_owned() };
    if service != "git-upload-pack" && service != "git-receive-pack" {
        return Err(git_service_error("smartHttpRequired", "Use a Git client that speaks the smart HTTP protocol.", 403));
    }
    let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let wrong = if target.route == "/info/refs" { *method != Method::GET } else { *method != Method::POST || content_type != format!("application/x-{service}-request") };
    if wrong {
        return Err(AppError::not_found());
    }
    let write = service == "git-receive-pack";
    let authorization = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let encoded = authorization.strip_prefix("Basic ").filter(|_| authorization.len() <= 2048).ok_or(AppError::new("unauthenticated", 401))?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).map(|d| String::from_utf8_lossy(&d).into_owned()).unwrap_or_default();
    let (user, secret) = decoded.split_once(':').filter(|(u, _)| !u.is_empty()).ok_or(AppError::new("unauthenticated", 401))?;
    if !crate::auth::is_token(secret) {
        return Err(AppError::new("unauthenticated", 401));
    }
    let auth = runtime.db.read("service_protocol_authenticate", json!([user, crate::auth::digest_token(secret), write])).await?;
    let session_id = auth["sessionId"].as_str().unwrap_or_default().to_owned();
    let repos = &runtime.git;
    let folder_id = match (&target.folder, &target.alias) {
        (Some(id), _) => id.clone(),
        (None, Some((alias_user, alias_path))) => {
            if alias_user.to_lowercase() != user.to_lowercase() {
                return Err(AppError::not_found());
            }
            repos.call(&session_id, "resolve", json!({ "path": alias_path })).await?["folderId"].as_str().unwrap_or_default().to_owned()
        }
        _ => return Err(AppError::not_found()),
    };
    let protocol = headers
        .get("git-protocol")
        .and_then(|v| v.to_str().ok())
        .filter(|p| !p.is_empty() && p.len() <= 256 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.=:-".contains(&b)))
        .unwrap_or_default()
        .to_owned();
    let mut response = if service == "git-upload-pack" {
        upload_pack(repos, &session_id, &folder_id, target.route == "/info/refs", &protocol, headers, body).await?
    } else if target.route == "/info/refs" {
        // receive-pack's advertisement: the refs as the database has them.
        let (_, state) = repos.sync(&session_id, &folder_id).await?;
        let mut refs: Vec<(String, String)> = state.main.iter().map(|m| (MAIN_REF.to_owned(), m.clone())).collect();
        refs.extend(state.refs.iter().map(|r| (r.name.clone(), r.sha.clone())));
        let capabilities = "report-status delete-refs side-band-64k quiet atomic ofs-delta object-format=sha1 agent=dar";
        let mut out = pkt(b"# service=git-receive-pack\n");
        out.extend_from_slice(FLUSH);
        if refs.is_empty() {
            out.extend(pkt(format!("{ZERO} capabilities^{{}}\0{capabilities}\n").as_bytes()));
        }
        for (index, (name, sha)) in refs.iter().enumerate() {
            let caps = if index == 0 { format!("\0{capabilities}") } else { String::new() };
            out.extend(pkt(format!("{sha} {name}{caps}\n").as_bytes()));
        }
        out.extend_from_slice(FLUSH);
        ([(header::CONTENT_TYPE, "application/x-git-receive-pack-advertisement")], out).into_response()
    } else {
        receive_pack(runtime.clone(), &session_id, &folder_id, headers, body).await?
    };
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, max-age=0, must-revalidate"));
    Ok(response)
}

async fn upload_pack(repos: &Repositories, session_id: &str, folder_id: &str, advertise: bool, protocol: &str, headers: &HeaderMap, body: Body) -> Result<Response> {
    let (dir, _) = repos.sync(session_id, folder_id).await?;
    let mut args = vec!["upload-pack", "--stateless-rpc"];
    if advertise {
        args.push("--http-backend-info-refs");
    }
    args.push(path_str(&dir));
    let env: Vec<(&str, &str)> = if protocol.is_empty() { Vec::new() } else { vec![("GIT_PROTOCOL", protocol)] };
    let mut child = git_command(&dir, &args, &env).stderr(Stdio::null()).spawn().map_err(|_| unavailable())?;
    let mut stdin = child.stdin.take().expect("stdin");
    if advertise {
        drop(stdin);
    } else {
        let mut source = body_stream(headers, body)?;
        tokio::spawn(async move {
            while let Some(chunk) = source.next().await {
                let Ok(chunk) = chunk else { return };
                if stdin.write_all(&chunk).await.is_err() {
                    return;
                }
            }
        });
    }
    let stdout = child.stdout.take().expect("stdout");
    let mut prefix = Vec::new();
    if advertise && !protocol.contains("version=2") {
        prefix.extend(pkt(b"# service=git-upload-pack\n"));
        prefix.extend_from_slice(FLUSH);
    }
    let output = stream::once(async move { Ok::<_, io::Error>(Bytes::from(prefix)) }).chain(with_exit(ReaderStream::new(stdout), child));
    let kind = if advertise { "application/x-git-upload-pack-advertisement" } else { "application/x-git-upload-pack-result" };
    Ok(([(header::CONTENT_TYPE, kind)], Body::from_stream(output)).into_response())
}

#[derive(Clone)]
pub struct RefCommand {
    pub old: String,
    pub new: String,
    pub name: String,
}

/// Database refusals as the pusher sees them.
fn push_failure(code: &str) -> Option<&'static str> {
    Some(match code {
        "quotaExceeded" => "not enough storage left in Dar",
        "conflict" => "the folder changed during the push; fetch and try again",
        "operationTooLarge" => "the push is too large for Dar; push fewer commits at a time",
        "forbidden" => "you can read this folder but not push to it",
        "notFound" => "this folder does not keep history",
        "invalidInput" => "Dar could not read the push",
        "contentUnavailable" => "some stored content is missing",
        _ => return None,
    })
}

/// Commit and tag header problems Git itself accepts by default; tree problems stay errors.
fn fsck_ignored() -> String {
    [
        "badDate", "badDateOverflow", "badEmail", "badName", "badTimezone", "missingAuthor", "missingCommitter", "missingEmail", "missingNameBeforeEmail", "missingSpaceBeforeDate", "missingSpaceBeforeEmail", "zeroPaddedDate", "multipleAuthors",
        "missingTaggerEntry", "badTagName",
    ]
    .iter()
    .map(|id| format!("{id}=ignore"))
    .collect::<Vec<_>>()
    .join(",")
}

struct Outcome {
    results: HashMap<String, Option<String>>,
    messages: Vec<String>,
}

/// git-receive-pack: quarantine the pack, read what it adds, let the database decide, then report per ref.
async fn receive_pack(runtime: crate::http::Shared, session_id: &str, folder_id: &str, headers: &HeaderMap, body: Body) -> Result<Response> {
    let mut reader = PacketReader { buffer: Vec::new(), done: false, source: body_stream(headers, body)? };
    let mut commands: Vec<RefCommand> = Vec::new();
    let mut capabilities: HashSet<String> = HashSet::new();
    while let Some(packet) = reader.packet().await? {
        let mut text = String::from_utf8_lossy(&packet).into_owned();
        if let Some(nul) = text.find('\0') {
            if commands.is_empty() {
                capabilities = text[nul + 1..].trim().split(' ').map(str::to_owned).collect();
            }
            text.truncate(nul);
        }
        if text.starts_with("shallow ") {
            return Err(AppError::with("shallowPush", 400, "Dar cannot take a push from a shallow clone."));
        }
        let parts: Vec<&str> = text.trim_end_matches('\n').split(' ').collect();
        let (old, new, name) = (parts.first().copied().unwrap_or_default(), parts.get(1).copied().unwrap_or_default(), parts.get(2).copied().unwrap_or_default());
        if !is_sha(old) || !is_sha(new) || name.is_empty() || commands.len() >= 1000 {
            return Err(AppError::with("invalidRequest", 400, "The Git request is malformed."));
        }
        commands.push(RefCommand { old: old.into(), new: new.into(), name: name.into() });
    }
    let side_band = capabilities.contains("side-band-64k");
    let report = capabilities.contains("report-status");
    let atomic = capabilities.contains("atomic");
    let (dir, _) = runtime.git.sync(session_id, folder_id).await?;
    let quarantine = dir.join("objects").join(format!("incoming-{}", crate::auth::uuid()));
    let mut unpack = "ok".to_owned();
    let mut early_error: Option<String> = None;
    let unpacked = async {
        create_private_dir(&quarantine.join("pack")).await?;
        if commands.iter().any(|c| c.new != ZERO) {
            // Thin packs are completed from the repository; --strict checks every object (and refuses .git paths).
            let strict = format!("--strict={}", fsck_ignored());
            let max = format!("--max-input-size={PACK_BYTES}");
            let env = quarantine_env(&dir, &quarantine);
            let env_ref: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let mut child = git_command(&dir, &["index-pack", "--stdin", "--fix-thin", &strict, &max], &env_ref).spawn().map_err(|_| unavailable())?;
            let mut stdin = child.stdin.take().expect("stdin");
            let mut rest = reader.rest();
            let feeding = async {
                while let Some(chunk) = rest.next().await {
                    let Ok(chunk) = chunk else { break };
                    if stdin.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
                drop(stdin);
            };
            let (_, output) = tokio::join!(feeding, child.wait_with_output());
            let output = output.map_err(|_| unavailable())?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let reason = stderr.lines().find_map(|l| l.split_once("fatal: ").map(|(_, m)| m.chars().take(200).collect::<String>()));
                unpack = reason.unwrap_or_else(|| "the pack could not be read".into());
            }
        } else {
            let mut rest = reader.rest();
            while rest.next().await.is_some() {}
        }
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(error) = unpacked {
        early_error = Some(failure_text(&error));
    }
    let runtime2 = runtime.clone();
    let (session, folder) = (session_id.to_owned(), folder_id.to_owned());
    let commands2 = commands.clone();
    let apply = async move {
        let mut results: HashMap<String, Option<String>> = HashMap::new();
        let mut messages: Vec<String> = Vec::new();
        if let Some(message) = early_error {
            for c in &commands2 {
                results.insert(c.name.clone(), Some(message.clone()));
            }
            messages.push(format!("Dar: {message}"));
        } else if unpack != "ok" {
            for c in &commands2 {
                results.insert(c.name.clone(), Some("unpacker error".into()));
            }
        } else {
            let _guard = runtime2.git.lock(&folder).await;
            let env = quarantine_env(&dir, &quarantine);
            match apply_push(&runtime2, &session, &folder, &dir, &env, &commands2, atomic, &quarantine).await {
                Ok(outcome) => {
                    results = outcome.results;
                    messages = outcome.messages;
                }
                Err(error) => {
                    let message = failure_text(&error);
                    for c in &commands2 {
                        results.insert(c.name.clone(), Some(message.clone()));
                    }
                    messages.push(format!("Dar: {message}"));
                }
            }
        }
        let _ = tokio::fs::remove_dir_all(&quarantine).await;
        let mut status = pkt(format!("unpack {unpack}\n").as_bytes());
        for c in &commands2 {
            let line = match results.get(&c.name).cloned().flatten() {
                Some(error) => format!("ng {} {}\n", c.name, error.replace('\n', " ")),
                None => format!("ok {}\n", c.name),
            };
            status.extend(pkt(line.as_bytes()));
        }
        status.extend_from_slice(FLUSH);
        let mut out = Vec::new();
        if !report {
            return out;
        }
        if !side_band {
            return status;
        }
        for message in messages {
            let mut band = vec![2u8];
            band.extend_from_slice(format!("{message}\n").as_bytes());
            out.extend(pkt(&band));
        }
        for chunk in status.chunks(65515) {
            let mut band = vec![1u8];
            band.extend_from_slice(chunk);
            out.extend(pkt(&band));
        }
        out.extend_from_slice(FLUSH);
        out
    };
    let kind = [(header::CONTENT_TYPE, "application/x-git-receive-pack-result")];
    if !(side_band && report) {
        return Ok((kind, apply.await).into_response());
    }
    // Checking and storing a large push can take a while with nothing to say: an empty side-band
    // packet every ten seconds keeps proxies from giving up.
    let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(4);
    tokio::spawn(async move {
        let keep = Bytes::from(pkt(&[1u8]));
        let _ = sender.send(keep.clone()).await;
        let mut work = Box::pin(apply);
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        ticker.tick().await;
        loop {
            tokio::select! {
                out = &mut work => {
                    let _ = sender.send(Bytes::from(out)).await;
                    break;
                }
                _ = ticker.tick() => {
                    let _ = sender.send(keep.clone()).await;
                }
            }
        }
    });
    let output = stream::unfold(receiver, |mut r| async move { r.recv().await.map(|b| (Ok::<_, io::Error>(b), r)) });
    Ok((kind, Body::from_stream(output)).into_response())
}

fn quarantine_env(dir: &Path, quarantine: &Path) -> Vec<(String, String)> {
    vec![
        ("GIT_DIR".into(), path_str(dir).into()),
        ("GIT_OBJECT_DIRECTORY".into(), path_str(quarantine).into()),
        ("GIT_ALTERNATE_OBJECT_DIRECTORIES".into(), path_str(&dir.join("objects")).into()),
        ("GIT_QUARANTINE_PATH".into(), path_str(quarantine).into()),
    ]
}

fn failure_text(error: &AppError) -> String {
    if error.status < 500 {
        push_failure(&error.code).map(str::to_owned).unwrap_or_else(|| error.message.clone())
    } else if error.code == "quotaExceeded" {
        push_failure("quotaExceeded").expect("message").into()
    } else {
        "the push could not be stored".into()
    }
}

struct ParsedCommit {
    sha: String,
    raw: Vec<u8>,
    parents: Vec<String>,
    author: String,
    message: String,
    time: f64,
    changes: Vec<Change>,
}

#[derive(Clone)]
struct Change {
    path: String,
    previous_path: String,
    kind: &'static str,
    mode: String,
    previous_mode: String,
    blob: String,
    previous_blob: String,
    media_type: String,
}

impl Change {
    fn json(&self) -> Value {
        json!({ "path": self.path, "previousPath": self.previous_path, "kind": self.kind, "mode": self.mode, "previousMode": self.previous_mode, "blob": self.blob, "previousBlob": self.previous_blob, "mediaType": self.media_type })
    }
}

fn mode_of(value: &str) -> String {
    if value == "100644" { String::new() } else { value.to_owned() }
}

fn utf16_prefix(value: &str, units: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(units).collect::<Vec<_>>())
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as f64
}

/// `git log`'s author text without its address.
fn without_email(value: &str) -> String {
    match value.find('<') {
        Some(i) if value[i..].contains('>') => value[..i].trim().to_owned(),
        _ => value.trim().to_owned(),
    }
}

fn parse_commit(sha: &str, raw: Vec<u8>) -> Result<ParsedCommit> {
    if raw.len() > OBJECT_BYTES {
        return Err(AppError::with("objectTooLarge", 413, &format!("Commit {} is too large for Dar.", &sha[..7])));
    }
    let header = header_of(&raw);
    let (mut parents, mut author, mut time) = (Vec::new(), String::new(), 0.0);
    for line in &header {
        if let Some(p) = line.strip_prefix("parent ") {
            parents.push(p.to_owned());
        } else if let Some(a) = line.strip_prefix("author ") {
            author = without_email(a);
        } else if let Some(c) = line.strip_prefix("committer ") {
            // >\s*(\d+)\s*[+-]\d{4}\s*$
            if let Some(after) = c.rsplit_once('>').map(|(_, a)| a.trim()) {
                let mut parts = after.split_whitespace();
                let (seconds, zone) = (parts.next(), parts.next());
                let zone_ok = zone.is_some_and(|z| z.len() == 5 && (z.starts_with('+') || z.starts_with('-')) && z[1..].bytes().all(|b| b.is_ascii_digit()));
                if let Some(s) = seconds.filter(|s| s.bytes().all(|b| b.is_ascii_digit()) && zone_ok && parts.next().is_none()) {
                    time = s.parse::<f64>().unwrap_or(0.0) * 1000.0;
                }
            }
        }
    }
    let end = raw.windows(2).position(|w| w == b"\n\n");
    let message = end.map(|e| String::from_utf8_lossy(&raw[e + 2..]).replace('\u{fffd}', "")).unwrap_or_default().trim().to_owned();
    let message = if message.is_empty() { "(no message)".to_owned() } else { message };
    let author = utf16_prefix(&author, 256);
    Ok(ParsedCommit {
        sha: sha.into(),
        raw,
        parents,
        author: if author.is_empty() { "Unknown".into() } else { author },
        message: utf16_prefix(&message, MESSAGE_UNITS),
        time: if time == 0.0 { now_ms() } else { time },
        changes: Vec::new(),
    })
}

/// Parses `git diff-tree --stdin -r -z --raw --always` output into each commit's changes, in input order.
fn parse_diff(output: &[u8]) -> Result<Vec<Vec<Change>>> {
    let text = String::from_utf8_lossy(output);
    let tokens: Vec<&str> = text.split('\0').collect();
    let mut result: Vec<Vec<Change>> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        if token.is_empty() {
            i += 1;
            continue;
        }
        let Some(meta) = token.strip_prefix(':') else {
            result.push(Vec::new());
            i += 1;
            continue;
        };
        let fields: Vec<&str> = meta.split(' ').collect();
        if fields.len() < 5 {
            return Err(AppError::with("gitFailed", 502, "Git could not read the push."));
        }
        let (old_mode, new_mode, old_sha, new_sha, status) = (fields[0], fields[1], fields[2], fields[3], fields[4]);
        if old_mode == "160000" || new_mode == "160000" {
            return Err(AppError::with("unsupportedSubmodule", 400, "Dar does not keep Git submodules."));
        }
        for mode in [old_mode, new_mode] {
            if !["000000", "100644", "100755", "120000"].contains(&mode) {
                return Err(AppError::with("unsupportedMode", 400, &format!("Dar cannot keep files with Git mode {mode}.")));
            }
        }
        i += 1;
        let first = tokens.get(i).copied().unwrap_or_default().to_owned();
        let current = result.last_mut().ok_or_else(|| AppError::with("gitFailed", 502, "Git could not read the push."))?;
        let base = Change { path: first.clone(), previous_path: String::new(), kind: "modified", mode: mode_of(new_mode), previous_mode: mode_of(old_mode), blob: new_sha.into(), previous_blob: old_sha.into(), media_type: String::new() };
        match status.chars().next() {
            Some(k @ ('R' | 'C')) => {
                i += 1;
                let second = tokens.get(i).copied().unwrap_or_default().to_owned();
                if k == 'R' {
                    current.push(Change { path: second, previous_path: first, kind: "renamed", ..base });
                } else {
                    current.push(Change { path: second, kind: "added", previous_mode: String::new(), previous_blob: String::new(), ..base });
                }
            }
            Some('A') => current.push(Change { kind: "added", previous_mode: String::new(), previous_blob: String::new(), ..base }),
            Some('D') => current.push(Change { kind: "deleted", mode: String::new(), blob: String::new(), ..base }),
            _ => current.push(base),
        }
        i += 1;
    }
    Ok(result)
}

/// Media types by extension, for files written outside a browser upload (`media-types.ts`).
pub fn media_type_of(name: &str) -> &'static str {
    let extension = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).filter(|e| !e.is_empty() && e.bytes().all(|b| b.is_ascii_alphanumeric()));
    match extension.as_deref() {
        Some("md" | "markdown") => "text/markdown",
        Some("txt") => "text/plain",
        Some("csv") => "text/csv",
        Some("html") => "text/html",
        Some("css") => "text/css",
        Some("js" | "mjs") => "text/javascript",
        Some("ts" | "tsx") => "text/plain",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn utf8_sample(sample: &[u8]) -> bool {
    std::str::from_utf8(sample).is_ok() || sample.len() == 8192 && (1..=3).any(|cut| std::str::from_utf8(&sample[..8192 - cut]).is_ok())
}

async fn batch_check(dir: &Path, env: &[(&str, &str)], shas: &[String]) -> Result<Vec<String>> {
    let input = format!("{}\n", shas.join("\n"));
    Ok(lines(&git_full(dir, &["cat-file", "--batch-check"], env, input.as_bytes(), false, Duration::from_secs(300)).await?.stdout))
}

/// Reads the pushed objects with Git and hands the push to the database; returns each ref's error, if any.
#[allow(clippy::too_many_arguments)]
async fn apply_push(runtime: &crate::http::Runtime, session_id: &str, folder_id: &str, dir: &Path, env: &[(String, String)], commands: &[RefCommand], atomic: bool, quarantine: &Path) -> Result<Outcome> {
    let repos = &runtime.git;
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let run = async |args: &[&str], input: &[u8]| git_full(dir, args, &env, input, false, Duration::from_secs(300)).await;
    let (_, state) = repos.sync_locked(session_id, folder_id).await?;
    let mut results: HashMap<String, Option<String>> = HashMap::new();
    let mut messages: Vec<String> = Vec::new();
    let mut tips: Vec<String> = Vec::new();
    for c in commands.iter().filter(|c| c.new != ZERO) {
        if !tips.contains(&c.new) {
            tips.push(c.new.clone());
        }
    }
    let mut types: HashMap<String, String> = HashMap::new();
    if !tips.is_empty() {
        for line in batch_check(dir, &env, &tips).await? {
            let parts: Vec<&str> = line.split(' ').collect();
            types.insert(parts[0].into(), parts.get(1).copied().unwrap_or_default().into());
        }
    }
    // Annotated tags: keep each tag object down to the commit it names.
    let mut tags: Vec<Value> = Vec::new();
    let mut commit_tips: Vec<String> = Vec::new();
    for tip in &tips {
        let (mut sha, mut kind) = (tip.clone(), types.get(tip).cloned().unwrap_or_default());
        let mut depth = 0;
        while kind == "tag" && depth < 16 {
            let raw = run(&["cat-file", "tag", &sha], b"").await?.stdout;
            if raw.len() > OBJECT_BYTES {
                return Err(AppError::with("objectTooLarge", 413, "A tag is too large for Dar."));
            }
            let header = header_of(&raw);
            let find = |prefix: &str| header.iter().find_map(|l| l.strip_prefix(prefix).map(str::to_owned)).unwrap_or_default();
            let target = find("object ");
            let target_type = find("type ");
            let tagger = header.iter().find_map(|l| l.strip_prefix("tagger ")).map(without_email).unwrap_or_default();
            tags.push(json!({ "sha": sha, "raw": base64::engine::general_purpose::STANDARD.encode(&raw), "target": target, "author": utf16_prefix(&tagger, 256) }));
            sha = target;
            kind = target_type;
            depth += 1;
        }
        if kind == "commit" && !commit_tips.contains(&sha) {
            commit_tips.push(sha);
        }
    }
    // New commits: everything the tips reach that no ref here reaches yet, parents first.
    let fresh = if commit_tips.is_empty() {
        Vec::new()
    } else {
        let mut args = vec!["rev-list", "--topo-order", "--reverse"];
        args.extend(commit_tips.iter().map(String::as_str));
        args.extend(["--not", "--all"]);
        lines(&run(&args, b"").await?.stdout)
    };
    if fresh.len() > COMMITS {
        return Err(AppError::with("pushTooLarge", 413, &format!("Push at most {COMMITS} new commits at a time.")));
    }
    let mut order: Vec<String> = Vec::new();
    let mut commits: HashMap<String, ParsedCommit> = HashMap::new();
    if !fresh.is_empty() {
        let mut child = git_command(dir, &["cat-file", "--batch"], &env).stderr(Stdio::null()).spawn().map_err(|_| unavailable())?;
        let mut stdin = child.stdin.take().expect("stdin");
        let input = format!("{}\n", fresh.join("\n"));
        tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
        });
        let mut batch = BatchReader { reader: BufReader::new(child.stdout.take().expect("stdout")) };
        for sha in &fresh {
            let header = batch.header().await?;
            if header.kind != "commit" {
                return Err(AppError::with("invalidRequest", 400, "The push is malformed."));
            }
            let raw = batch.read(header.size).await?;
            commits.insert(sha.clone(), parse_commit(sha, raw)?);
            order.push(sha.clone());
        }
    }
    // main's new commits, oldest first, and their changes against main's previous commit.
    let current_main = state.main.clone().unwrap_or_else(|| ZERO.into());
    let main_command = commands.iter().find(|c| c.name == MAIN_REF && c.new != ZERO && c.old == current_main && types.get(&c.new).map(String::as_str) == Some("commit")).cloned();
    let chain: Vec<String> = match &main_command {
        Some(c) => {
            let mut args = vec!["rev-list", "--first-parent", "--reverse", c.new.as_str()];
            if let Some(main) = &state.main {
                args.extend(["--not", main.as_str()]);
            }
            lines(&run(&args, b"").await?.stdout)
        }
        None => Vec::new(),
    };
    if chain.len() > COMMITS {
        return Err(AppError::with("pushTooLarge", 413, &format!("Push at most {COMMITS} new commits at a time.")));
    }
    let mut diffs: Vec<String> = Vec::new();
    let mut targets: Vec<(bool, String)> = Vec::new();
    for sha in &order {
        let commit = &commits[sha];
        diffs.push(match commit.parents.first() {
            Some(p) => format!("{sha} {p}"),
            None => sha.clone(),
        });
        targets.push((true, sha.clone()));
    }
    for (index, sha) in chain.iter().enumerate() {
        let base = if index > 0 { Some(chain[index - 1].clone()) } else { state.main.clone() };
        if let Some(commit) = commits.get(sha)
            && (commit.parents.first() == base.as_ref() || base.is_none() && commit.parents.is_empty())
        {
            continue;
        }
        diffs.push(match &base {
            Some(b) => format!("{sha} {b}"),
            None => sha.clone(),
        });
        targets.push((false, sha.clone()));
    }
    let mut chain_changes: HashMap<String, Vec<Change>> = HashMap::new();
    if !diffs.is_empty() {
        let input = format!("{}\n", diffs.join("\n"));
        let parsed = parse_diff(&run(&["diff-tree", "--stdin", "-r", "-z", "-M", "--raw", "--root", "--always"], input.as_bytes()).await?.stdout)?;
        if parsed.len() != targets.len() {
            return Err(AppError::with("gitFailed", 502, "Git could not read the push."));
        }
        for ((is_commit, sha), changes) in targets.into_iter().zip(parsed) {
            if is_commit {
                commits.get_mut(&sha).expect("commit").changes = changes;
            } else {
                chain_changes.insert(sha, changes);
            }
        }
    }
    let all_changes: Vec<&Change> = commits.values().flat_map(|c| c.changes.iter()).chain(chain_changes.values().flatten()).collect();
    if all_changes.len() > CHANGES {
        return Err(AppError::with("pushTooLarge", 413, "This push changes too many files at once; push fewer commits at a time."));
    }
    // Content: sizes from Git; the database says which it already has.
    let mut wanted: Vec<String> = Vec::new();
    for change in all_changes.iter().filter(|c| c.kind != "deleted") {
        if !wanted.contains(&change.blob) {
            wanted.push(change.blob.clone());
        }
    }
    let mut sizes: HashMap<String, u64> = HashMap::new();
    if !wanted.is_empty() {
        for line in batch_check(dir, &env, &wanted).await? {
            let parts: Vec<&str> = line.split(' ').collect();
            if parts.get(1) != Some(&"blob") {
                return Err(AppError::with("invalidRequest", 400, "The push is missing objects."));
            }
            let size: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            if size > MAX_FILE_BYTES {
                return Err(AppError::with("uploadTooLarge", 413, "A file in this push is larger than Dar allows."));
            }
            sizes.insert(parts[0].into(), size);
        }
    }
    let blobs_in: Vec<Value> = wanted.iter().map(|sha| json!({ "sha": sha, "size": sizes.get(sha).copied().unwrap_or(0) })).collect();
    let begin = repos.call(session_id, "pushBegin", json!({ "folderId": folder_id, "blobs": blobs_in })).await?;
    let rows = begin["blobs"].as_array().cloned().unwrap_or_default();
    let mut text: HashMap<String, bool> = HashMap::new();
    let mut stored: Vec<Value> = Vec::new();
    let uploads: Vec<(String, String)> = rows.iter().filter_map(|r| Some((r["sha"].as_str()?.to_owned(), r["uploadId"].as_str()?.to_owned()))).collect();
    for row in &rows {
        if let (Some(sha), Some(blob_id)) = (row["sha"].as_str(), row["blobId"].as_str()) {
            stored.push(json!({ "sha": sha, "size": sizes.get(sha).copied().unwrap_or(0), "blobId": blob_id }));
        }
    }
    if !uploads.is_empty() {
        let mut child = git_command(dir, &["cat-file", "--batch"], &env).stderr(Stdio::null()).spawn().map_err(|_| unavailable())?;
        let mut stdin = child.stdin.take().expect("stdin");
        let input = format!("{}\n", uploads.iter().map(|u| u.0.as_str()).collect::<Vec<_>>().join("\n"));
        tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
        });
        let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
        for (sha, upload_id) in &uploads {
            let size = sizes.get(sha).copied().unwrap_or(0);
            let header = {
                let mut batch = BatchReader { reader };
                let header = batch.header().await?;
                reader = batch.reader;
                header
            };
            if header.sha != *sha || header.size as u64 != size {
                return Err(AppError::with("gitFailed", 502, "Git could not read the push."));
            }
            let mut sample: Option<Vec<u8>> = None;
            let digest = {
                let source = stream::unfold((&mut reader, size, &mut sample), |(r, left, sample)| async move {
                    if left == 0 {
                        return None;
                    }
                    let mut buffer = vec![0u8; left.min(64 * 1024) as usize];
                    match r.read(&mut buffer).await {
                        Ok(0) | Err(_) => Some((Err(io::Error::other("short")), (r, 0, sample))),
                        Ok(n) => {
                            buffer.truncate(n);
                            if sample.is_none() {
                                *sample = Some(buffer[..n.min(8192)].to_vec());
                            }
                            Some((Ok(Bytes::from(buffer)), (r, left - n as u64, sample)))
                        }
                    }
                });
                runtime.blobs.receive(Box::pin(source), size).await?
            };
            let mut newline = [0u8; 1];
            reader.read_exact(&mut newline).await.map_err(|_| BatchReader::broken())?;
            // UTF-8 at the start (cut at a character boundary) makes an unknown extension plain text.
            text.insert(sha.clone(), sample.as_ref().is_some_and(|s| utf8_sample(s)));
            stored.push(json!({ "sha": sha, "size": size, "uploadId": upload_id, "digest": digest }));
        }
    }
    // By extension; else plain text for UTF-8 content; else (""), the type stored content already has.
    let typed = |change: &Change| -> Value {
        if change.kind == "deleted" {
            return change.json();
        }
        let guess = media_type_of(&change.path);
        let media = if guess != "application/octet-stream" { guess } else if text.get(&change.blob) == Some(&true) { "text/plain" } else { "" };
        Change { media_type: media.into(), ..change.clone() }.json()
    };
    let mut document = json!({
        "folderId": folder_id,
        "atomic": atomic,
        "updates": commands.iter().map(|c| json!({ "old": c.old, "new": c.new, "name": c.name })).collect::<Vec<_>>(),
        "commits": order.iter().map(|sha| { let c = &commits[sha]; json!({ "sha": c.sha, "raw": base64::engine::general_purpose::STANDARD.encode(&c.raw), "parents": c.parents, "author": c.author, "message": c.message, "time": c.time, "changes": c.changes.iter().map(typed).collect::<Vec<_>>() }) }).collect::<Vec<_>>(),
        "tags": tags,
        "blobs": stored,
    });
    if main_command.is_some() {
        document["main"] = json!({ "chain": chain.iter().map(|sha| match chain_changes.get(sha) {
            Some(changes) => json!({ "sha": sha, "changes": changes.iter().map(typed).collect::<Vec<_>>() }),
            None => json!({ "sha": sha }),
        }).collect::<Vec<_>>() });
    }
    let outcome = repos.call(session_id, "push", document).await?;
    for row in outcome["results"].as_array().cloned().unwrap_or_default() {
        results.insert(row["name"].as_str().unwrap_or_default().to_owned(), row["error"].as_str().map(str::to_owned));
    }
    // Keep the objects: refused refs leave them unreachable, which is harmless.
    let pack = quarantine.join("pack");
    let mut names: Vec<String> = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&pack).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort_by_key(|n| n.ends_with(".idx"));
    let target = dir.join("objects").join("pack");
    let _ = tokio::fs::create_dir_all(&target).await;
    for name in names {
        tokio::fs::rename(pack.join(&name), target.join(&name)).await.map_err(|_| AppError::internal())?;
    }
    let _ = repos.sync_locked(session_id, folder_id).await;
    if main_command.is_some() && results.get(MAIN_REF).cloned().flatten().is_none() {
        messages.push("Dar: the folder now matches main.".into());
    }
    Ok(Outcome { results, messages })
}

/// A ref change made in Dar, such as a merge from the Git app: checked and applied exactly like a
/// push of the same refs, atomically. The caller holds the folder's lock.
pub async fn push_from_dar(runtime: &crate::http::Runtime, session_id: &str, folder_id: &str, dir: &Path, commands: &[RefCommand]) -> Result<HashMap<String, Option<String>>> {
    let quarantine = dir.join("objects").join(format!("incoming-{}", crate::auth::uuid()));
    create_private_dir(&quarantine.join("pack")).await?;
    let env = vec![("GIT_DIR".to_owned(), path_str(dir).to_owned())];
    let result = apply_push(runtime, session_id, folder_id, dir, &env, commands, true, &quarantine).await;
    let _ = tokio::fs::remove_dir_all(&quarantine).await;
    Ok(result?.results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_packets() {
        let t = target_of("/dav/git/0a0a0a0a-0a0a-4a0a-8a0a-0a0a0a0a0a0a.git/info/refs").unwrap();
        assert_eq!(t.folder.as_deref(), Some("0a0a0a0a-0a0a-4a0a-8a0a-0a0a0a0a0a0a"));
        let t = target_of("/dav/git/amina/Projects/My%20site.git/git-upload-pack").unwrap();
        assert_eq!(t.alias, Some(("amina".into(), "Projects/My site".into())));
        assert!(target_of("/dav/git/amina/x.git/other").is_err());
        assert_eq!(pkt(b"# service=git-upload-pack\n"), b"001e# service=git-upload-pack\n");
        assert_eq!(media_type_of("README.MD"), "text/markdown");
        assert_eq!(media_type_of("a.bin"), "application/octet-stream");
    }

    #[test]
    fn diffs_and_commits() {
        let raw = b":000000 100644 0000000000000000000000000000000000000000 ce013625030ba8dba906f756967f9e9ca394464a A\0a.txt\0:100644 100644 ce013625030ba8dba906f756967f9e9ca394464a ce013625030ba8dba906f756967f9e9ca394464a R100\0a.txt\0b.txt\0";
        let mut output = b"1111111111111111111111111111111111111111\0".to_vec();
        output.extend_from_slice(raw);
        let parsed = parse_diff(&output).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!((parsed[0][0].kind, parsed[0][0].path.as_str()), ("added", "a.txt"));
        assert_eq!((parsed[0][1].kind, parsed[0][1].previous_path.as_str(), parsed[0][1].path.as_str()), ("renamed", "a.txt", "b.txt"));
        let commit = parse_commit("abcdefabcdefabcdefabcdefabcdefabcdefabcd", b"tree x\nauthor Amina <a@b> 1 +0000\ncommitter Amina <a@b> 1700000000 +0100\n\nHello\n".to_vec()).unwrap();
        assert_eq!((commit.author.as_str(), commit.time, commit.message.as_str()), ("Amina", 1_700_000_000_000.0, "Hello"));
    }
}
