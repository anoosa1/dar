//! The Git app's API under `/api/v1/git` (`runtime/git/api.ts`, docs/git.md). Repositories are
//! tracked folders; each read brings the folder's Git cache up to date under its lock and asks Git.
//! Deleting a branch or tag goes through the same database push as `git push --delete`.

use std::collections::HashMap;
use std::path::Path;

use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{StreamExt, stream};
use serde_json::{Map, Value, json};
use tokio_util::io::ReaderStream;

use crate::error::{AppError, Result};
use crate::git::{
    EMPTY_TREE, GitState, MAIN_REF, RefCommand, ZERO, encode_component, git_command, git_full,
    is_sha, lines, path_str, push_from_dar,
};
use crate::http::{Api, json_response};

const UNIT: char = '\x1f';
const RECORD: char = '\x1e';
const PAGE: usize = 50;
const MAX_PAGE: usize = 200;
const PATCH_FILE: usize = 256 * 1024;
const PATCH_TOTAL: usize = 2 * 1024 * 1024;
const PATCH_LINES: usize = 5000;
const README: u64 = 512 * 1024;
const BLOB: usize = 2 * 1024 * 1024;
const LAST_COMMITS: usize = 3000;
const COMPARE: usize = 250;
const EVENTS: usize = 30;
const TRACKER_READS: [&str; 3] = ["list", "issue", "labels"];

fn log_format() -> String {
    format!("--format={RECORD}%H{UNIT}%P{UNIT}%an{UNIT}%ae{UNIT}%at{UNIT}%D{UNIT}%s")
}

fn now_s() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs_f64()
}

/// Git in a repository with paths unquoted.
async fn run(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut all = vec!["-c", "core.quotepath=off", "--git-dir", path_str(dir)];
    all.extend_from_slice(args);
    Ok(git_full(
        dir,
        &all,
        &[],
        b"",
        false,
        std::time::Duration::from_secs(300),
    )
    .await?
    .stdout)
}
async fn run_in(dir: &Path, args: &[&str], input: &str) -> Result<Vec<u8>> {
    let mut all = vec!["-c", "core.quotepath=off", "--git-dir", path_str(dir)];
    all.extend_from_slice(args);
    Ok(git_full(
        dir,
        &all,
        &[],
        input.as_bytes(),
        false,
        std::time::Duration::from_secs(300),
    )
    .await?
    .stdout)
}
async fn plain(dir: &Path, args: &[&str]) -> Result<crate::git::Out> {
    let mut all = vec!["--git-dir", path_str(dir)];
    all.extend_from_slice(args);
    git_full(
        dir,
        &all,
        &[],
        b"",
        true,
        std::time::Duration::from_secs(300),
    )
    .await
}
fn text(out: &[u8]) -> String {
    String::from_utf8_lossy(out).trim().to_owned()
}

fn source_of(email: &str, subject: &str) -> &'static str {
    // Dar writes its own commits as <username>@dar.invalid; an Agent branch merge keeps its "Merge agent/…" message.
    if !email.ends_with("@dar.invalid") {
        "git"
    } else if subject.starts_with("Merge agent/") {
        "agent"
    } else {
        "dar"
    }
}

fn refs_of(decoration: &str) -> Vec<Value> {
    decoration
        .split(", ")
        .filter(|s| !s.is_empty())
        .filter_map(|item| {
            let name = item.strip_prefix("HEAD -> ").unwrap_or(item);
            if name == "HEAD" {
                return None;
            }
            Some(match name.strip_prefix("tag: ") {
                Some(tag) => json!({ "name": tag, "kind": "tag" }),
                None => json!({ "name": name, "kind": "branch" }),
            })
        })
        .collect()
}

fn number_before(stat: &str, word: &str) -> u64 {
    stat.find(word)
        .and_then(|i| stat[..i].trim_end().rsplit(' ').next()?.parse().ok())
        .unwrap_or(0)
}

/// `git log` with the log format and, optionally, --shortstat.
fn parse_log(output: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(output)
        .split(RECORD)
        .filter(|c| !c.is_empty())
        .map(|chunk| {
            let (head, stat) = chunk.split_once('\n').unwrap_or((chunk, ""));
            let f: Vec<&str> = head.split(UNIT).collect();
            let get = |i: usize| f.get(i).copied().unwrap_or_default();
            json!({ "sha": get(0), "parents": get(1).split(' ').filter(|p| !p.is_empty()).collect::<Vec<_>>(), "author": get(2), "time": get(4).parse::<f64>().unwrap_or(0.0) * 1000.0,
                "subject": get(6), "source": source_of(get(3), get(6)), "refs": refs_of(get(5)), "added": number_before(stat, " insertion"), "removed": number_before(stat, " deletion") })
        })
        .collect()
}

/// `![alt](src)` at the start of `value`: its length.
fn image_len(value: &str) -> Option<usize> {
    let rest = value.strip_prefix('!')?;
    skip_link(rest).map(|n| n + 1)
}

/// `[![alt](src)](href)` at the start of `value`: its length.
fn badge_len(value: &str) -> Option<usize> {
    let inner = value.strip_prefix('[')?;
    let image = image_len(inner)?;
    let after = inner[image..].strip_prefix(']')?;
    link_end(after).map(|n| 1 + image + 1 + n)
}

fn remove_all(value: &str, matcher: fn(&str) -> Option<usize>) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < value.len() {
        if let Some(n) = matcher(&value[i..]) {
            i += n;
            continue;
        }
        let c = value[i..].chars().next().expect("char");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Badges, then images, as `describe` in git/api.ts drops them.
fn strip_images(line: &str) -> String {
    remove_all(&remove_all(line, badge_len), image_len)
}

/// `[text](url)` at the start: its length.
fn skip_link(value: &str) -> Option<usize> {
    let inner = value.strip_prefix('[')?;
    let close = inner.find(']')?;
    link_end(&inner[close + 1..]).map(|n| 1 + close + 1 + n)
}
fn link_end(value: &str) -> Option<usize> {
    let inner = value.strip_prefix('(')?;
    Some(inner.find(')')? + 2)
}

/// `[text](url)` → text.
fn unlink(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('[') {
        match skip_link(&rest[start..]) {
            Some(n) => {
                let inner = &rest[start + 1..start + rest[start..].find(']').expect("bracket")];
                out.push_str(&rest[..start]);
                out.push_str(inner);
                rest = &rest[start + n..];
            }
            None => {
                out.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The README's first paragraph, without Markdown headings, badges or emphasis.
fn describe(value: &str) -> String {
    let mut paragraph: Vec<String> = Vec::new();
    for raw in value.replace('\r', "").split('\n') {
        let line = strip_images(raw).trim().to_owned();
        let skip = line.is_empty()
            || line.starts_with('#')
            || line.starts_with('<')
            || line.starts_with("```")
            || (!line.is_empty()
                && (line.chars().all(|c| c == '=') || line.chars().all(|c| c == '-')));
        if skip {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        paragraph.push(line);
    }
    let joined = unlink(&strip_images(&paragraph.join(" ")));
    let value: String = joined
        .chars()
        .filter(|c| !"*_`".contains(*c))
        .collect::<String>()
        .trim()
        .to_owned();
    let units: Vec<u16> = value.encode_utf16().collect();
    if units.len() > 300 {
        format!("{}…", String::from_utf16_lossy(&units[..299]))
    } else {
        value
    }
}

fn readme_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "readme" || ["readme.md", "readme.markdown", "readme.txt"].contains(&lower.as_str())
}

/// A path inside a tree: no empty, dot or dot-dot parts.
fn tree_path(value: Option<&String>) -> Result<String> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(String::new());
    };
    if value.len() > 4096
        || value
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".." || p.contains('\0'))
    {
        return Err(AppError::invalid());
    }
    Ok(value.clone())
}

/// `validRefName` in shared/git.ts.
pub fn valid_ref_name(name: &str) -> bool {
    let bad = name
        .chars()
        .any(|c| (c as u32) <= 0x20 || c as u32 == 0x7f || "~^:?*[\\".contains(c))
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name
            .split('/')
            .any(|p| p.ends_with(".lock") || p.starts_with('.'));
    !name.is_empty()
        && name.encode_utf16().count() <= 200
        && !bad
        && name != "HEAD"
        && name != "@"
        && !name.starts_with('-')
}

fn git_alias(username: &str, path: &str) -> String {
    format!(
        "/dav/git/{}/{}.git",
        encode_component(username),
        path.split('/')
            .map(encode_component)
            .collect::<Vec<_>>()
            .join("/")
    )
}

/// A branch or tag the repository has, as a full ref; anything else is not found.
async fn ref_of(dir: &Path, name: Option<&str>, kinds: &[&str]) -> Result<String> {
    let Some(name) = name.filter(|n| valid_ref_name(n)) else {
        return Err(AppError::not_found());
    };
    for kind in kinds {
        let full = format!("refs/{kind}/{name}");
        if plain(dir, &["show-ref", "--verify", "--quiet", &full])
            .await?
            .code
            == 0
        {
            return Ok(full);
        }
    }
    Err(AppError::not_found())
}
const BOTH: [&str; 2] = ["heads", "tags"];

/// A full commit ID the repository has.
async fn commit_of(dir: &Path, value: &str) -> Result<String> {
    if !is_sha(value) {
        return Err(AppError::not_found());
    }
    let out = plain(dir, &["cat-file", "-t", value]).await?;
    if out.code != 0 || out.text() != "commit" {
        return Err(AppError::not_found());
    }
    Ok(value.to_owned())
}

struct Git<'a> {
    r: &'a Api<'a>,
}

impl Git<'_> {
    fn session(&self) -> &str {
        self.r.session_id()
    }
    async fn repo_rows(&self) -> Result<Vec<Value>> {
        Ok(self
            .r
            .runtime
            .git
            .call(self.session(), "repositories", json!({}))
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default())
    }
    async fn tracker(&self, operation: &str, document: Value) -> Result<Value> {
        self.r
            .runtime
            .db
            .operation(
                "service_git_tracker",
                operation,
                &TRACKER_READS,
                json!([self.session(), operation, document.to_string()]),
            )
            .await
    }
    async fn overview(&self, folder_id: &str) -> Result<Value> {
        self.r
            .runtime
            .db
            .operation(
                "service_history",
                "overview",
                &crate::history::READ_OPERATIONS,
                json!([
                    self.session(),
                    "overview",
                    json!({ "folderId": folder_id }).to_string()
                ]),
            )
            .await
    }

    async fn summary(&self, row: &Value) -> Value {
        let id = row["id"].as_str().unwrap_or_default();
        let origin = &self.r.runtime.config.origin;
        let alias = if row["role"] == "owner" {
            git_alias(self.r.username(), row["path"].as_str().unwrap_or_default())
        } else {
            format!("/dav/git/{id}.git")
        };
        let mut base = json!({ "id": id, "name": row["name"], "path": row["path"], "parentId": row["parentId"], "createdAt": row["createdAt"], "revision": row["revision"], "dirty": row["dirty"],
            "agentBranches": row["agentBranches"], "size": row["size"], "role": row["role"], "openIssues": row["openIssues"], "openPulls": row["openPulls"],
            "clone": { "alias": format!("{origin}{alias}"), "id": format!("{origin}/dav/git/{id}.git") } });
        if !row["owner"].is_null() {
            base["owner"] = row["owner"].clone();
        }
        let detail = async {
            let _guard = self.r.runtime.git.lock(id).await;
            let (dir, state) = self.r.runtime.git.sync_locked(self.session(), id).await?;
            let refs = lines(
                &run(
                    &dir,
                    &[
                        "for-each-ref",
                        "--format=%(refname)",
                        "refs/heads",
                        "refs/tags",
                    ],
                )
                .await?,
            );
            let commits: u64 = if state.main.is_some() {
                text(&run(&dir, &["rev-list", "--count", "refs/heads/main"]).await?)
                    .parse()
                    .unwrap_or(0)
            } else {
                0
            };
            let since = now_s() - 84.0 * 86400.0;
            let mut weeks = [0u64; 12];
            let mut updated = row["createdAt"].as_f64().unwrap_or(0.0);
            if !refs.is_empty() {
                let since_arg = format!("--since={}", since.floor() as i64);
                for value in lines(&run(&dir, &["log", "--all", &since_arg, "--format=%ct"]).await?)
                {
                    let index =
                        ((value.parse::<f64>().unwrap_or(0.0) - since) / (7.0 * 86400.0)).floor();
                    if (0.0..12.0).contains(&index) {
                        weeks[index as usize] += 1;
                    }
                }
                let newest: f64 = text(&run(&dir, &["log", "--all", "-1", "--format=%ct"]).await?)
                    .parse()
                    .unwrap_or(0.0);
                if newest != 0.0 {
                    updated = updated.max(newest * 1000.0);
                }
            }
            let mut description = String::new();
            if state.main.is_some() {
                let listing = lines(&run(&dir, &["ls-tree", "-l", "refs/heads/main"]).await?);
                let readme = listing.iter().find_map(|line| {
                    let (meta, name) = line.split_once('\t')?;
                    let parts: Vec<&str> = meta.split_whitespace().collect();
                    let size: u64 = parts.get(3)?.parse().ok()?;
                    (parts.get(1) == Some(&"blob") && readme_name(name) && size <= README)
                        .then(|| parts[2].to_owned())
                });
                if let Some(sha) = readme {
                    description = describe(&String::from_utf8_lossy(
                        &run(&dir, &["cat-file", "blob", &sha]).await?,
                    ));
                }
            }
            Ok::<Value, AppError>(
                json!({ "description": description, "commits": commits, "branches": refs.iter().filter(|r| r.starts_with("refs/heads/")).count(),
                "tags": refs.iter().filter(|r| r.starts_with("refs/tags/")).count(), "updatedAt": updated, "weeks": weeks }),
            )
        };
        match detail.await {
            Ok(Value::Object(extra)) => {
                for (k, v) in extra {
                    base[k] = v;
                }
            }
            Ok(_) => {}
            Err(cause) => {
                for (k, v) in [
                    ("description", json!("")),
                    ("commits", json!(0)),
                    ("branches", json!(0)),
                    ("tags", json!(0)),
                    ("updatedAt", row["modifiedAt"].clone()),
                    ("weeks", json!(vec![0; 12])),
                    ("error", json!(cause.message)),
                ] {
                    base[k] = v;
                }
            }
        }
        base
    }

    async fn repositories(&self) -> Result<Value> {
        let rows = self.repo_rows().await?;
        let mut result = Vec::new();
        for row in &rows {
            result.push(self.summary(row).await);
        }
        result.sort_by(|a, b| {
            b["updatedAt"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&a["updatedAt"].as_f64().unwrap_or(0.0))
        });
        Ok(Value::Array(result))
    }

    async fn row_of(&self, folder_id: &str) -> Result<Value> {
        self.repo_rows()
            .await?
            .into_iter()
            .find(|r| r["id"] == json!(folder_id))
            .ok_or_else(AppError::not_found)
    }

    async fn activity(&self) -> Result<Value> {
        let rows = self.repo_rows().await?;
        let since = format!("--since={}", (now_s() - 366.0 * 86400.0).floor() as i64);
        let (mut times, mut events): (Vec<f64>, Vec<Value>) = (Vec::new(), Vec::new());
        for row in &rows {
            let id = row["id"].as_str().unwrap_or_default();
            let work = async {
                let _guard = self.r.runtime.git.lock(id).await;
                let (dir, _) = self.r.runtime.git.sync_locked(self.session(), id).await?;
                if lines(
                    &run(
                        &dir,
                        &[
                            "for-each-ref",
                            "--count=1",
                            "--format=%(refname)",
                            "refs/heads",
                            "refs/tags",
                        ],
                    )
                    .await?,
                )
                .is_empty()
                {
                    return Ok::<(), AppError>(());
                }
                for value in lines(
                    &run(
                        &dir,
                        &["log", "--all", &since, "--max-count=20000", "--format=%at"],
                    )
                    .await?,
                ) {
                    times.push(value.parse().unwrap_or(0.0));
                }
                let max = format!("--max-count={EVENTS}");
                for commit in parse_log(
                    &run(
                        &dir,
                        &[
                            "log",
                            "--all",
                            "--date-order",
                            &max,
                            "--decorate=short",
                            "--decorate-refs=refs/heads/",
                            "--decorate-refs=refs/tags/",
                            &log_format(),
                        ],
                    )
                    .await?,
                ) {
                    events.push(json!({ "repoId": id, "repoName": row["name"], "sha": commit["sha"], "subject": commit["subject"], "author": commit["author"], "time": commit["time"], "source": commit["source"], "refs": commit["refs"] }));
                }
                Ok(())
            };
            let _ = work.await;
        }
        events.sort_by(|a, b| {
            b["time"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&a["time"].as_f64().unwrap_or(0.0))
        });
        events.truncate(EVENTS);
        Ok(json!({ "times": times, "events": events }))
    }
}

async fn tree(
    dir: &Path,
    state: &GitState,
    reference: Option<&String>,
    path_value: Option<&String>,
) -> Result<Value> {
    if state.main.is_none() && state.refs.is_empty() {
        return Ok(json!({ "ref": "main", "sha": "", "path": "", "entries": [] }));
    }
    let full = match reference {
        Some(r) if is_sha(r) => commit_of(dir, r).await?,
        Some(r) => ref_of(dir, Some(r), &BOTH).await?,
        None => MAIN_REF.to_owned(),
    };
    let path = tree_path(path_value)?;
    let sha = text(
        &run(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{full}^{{commit}}"),
            ],
        )
        .await?,
    );
    let folder = format!("{path}/");
    let mut args = vec!["ls-tree", "-z", "-l", sha.as_str(), "--"];
    if !path.is_empty() {
        args.push(&folder);
    }
    let listed = plain(dir, &args).await?;
    if listed.code != 0 {
        return Err(AppError::not_found());
    }
    let mut entries: Vec<Value> = String::from_utf8_lossy(&listed.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .filter_map(|item| {
            let (meta, full) = item.split_once('\t')?;
            let parts: Vec<&str> = meta.split_whitespace().collect();
            let (mode, kind, size) = (parts.first().copied()?, parts.get(1).copied()?, parts.get(3).copied().unwrap_or("-"));
            let kind = if kind == "tree" { "tree" } else if kind == "commit" { "submodule" } else if mode == "120000" { "link" } else { "blob" };
            Some(json!({ "name": full.rsplit('/').next().unwrap_or(full), "path": full, "type": kind, "size": size.parse::<u64>().unwrap_or(0) }))
        })
        .collect();
    if !path.is_empty() && entries.is_empty() {
        let kind = plain(dir, &["cat-file", "-t", &format!("{sha}:{path}")]).await?;
        if kind.text() != "tree" {
            return Err(AppError::not_found());
        }
    }
    entries.sort_by(|a, b| {
        let rank = |v: &Value| if v["type"] == "tree" { 0 } else { 1 };
        rank(a).cmp(&rank(b)).then_with(|| {
            crate::http::locale_compare(
                a["name"].as_str().unwrap_or_default(),
                b["name"].as_str().unwrap_or_default(),
            )
        })
    });
    // Each entry's newest commit: walk history once until every entry has one.
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    if !entries.is_empty() {
        let max = format!("--max-count={LAST_COMMITS}");
        let format = format!("--format={RECORD}%H{UNIT}%at{UNIT}%s");
        let scope = if path.is_empty() {
            ".".to_owned()
        } else {
            path.clone()
        };
        let output = run(
            dir,
            &[
                "log",
                &max,
                &format,
                "--name-only",
                "--no-renames",
                &sha,
                "--",
                &scope,
            ],
        )
        .await?;
        let mut index: HashMap<String, usize> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e["name"].as_str().unwrap_or_default().to_owned(), i))
            .collect();
        for chunk in String::from_utf8_lossy(&output).split(RECORD) {
            if index.is_empty() {
                break;
            }
            let mut parts = chunk.split('\n');
            let Some(head) = parts.next().filter(|h| !h.is_empty()) else {
                continue;
            };
            let f: Vec<&str> = head.split(UNIT).collect();
            for name in parts {
                let Some(rest) = name.strip_prefix(prefix.as_str()) else {
                    continue;
                };
                let first = rest.split('/').next().unwrap_or_default();
                if let Some(i) = index.remove(first) {
                    entries[i]["last"] = json!({ "sha": f.first(), "subject": f.get(2), "time": f.get(1).and_then(|t| t.parse::<f64>().ok()).unwrap_or(0.0) * 1000.0 });
                }
            }
        }
    }
    let readme = entries
        .iter()
        .find(|e| {
            e["type"] == "blob"
                && readme_name(e["name"].as_str().unwrap_or_default())
                && e["size"].as_u64().unwrap_or(0) <= README
        })
        .map(|e| e["path"].as_str().unwrap_or_default().to_owned());
    let short = full
        .strip_prefix("refs/heads/")
        .or_else(|| full.strip_prefix("refs/tags/"))
        .unwrap_or(&full)
        .to_owned();
    let mut value = json!({ "ref": short, "sha": sha, "path": path, "entries": entries });
    if let Some(readme) = readme {
        let content = String::from_utf8_lossy(
            &run(dir, &["cat-file", "blob", &format!("{sha}:{readme}")]).await?,
        )
        .into_owned();
        value["readme"] = json!({ "path": readme, "text": content });
    }
    Ok(value)
}

/// Changed files between two trees, with line counts and, while they fit, unified hunks.
async fn changes(dir: &Path, from: &str, to: &str, patches: bool) -> Result<Vec<Value>> {
    let numstat = String::from_utf8_lossy(
        &run(dir, &["diff-tree", "-r", "-M", "--numstat", "-z", from, to]).await?,
    )
    .into_owned();
    let status = String::from_utf8_lossy(
        &run(
            dir,
            &["diff-tree", "-r", "-M", "--name-status", "-z", from, to],
        )
        .await?,
    )
    .into_owned();
    let status: Vec<&str> = status.split('\0').collect();
    let numstat: Vec<&str> = numstat.split('\0').collect();
    let mut files: Vec<Value> = Vec::new();
    let mut i = 0;
    while i + 1 < status.len() {
        let code = status[i];
        i += 1;
        let kind = if code.starts_with('R') {
            "renamed"
        } else if code == "A" {
            "added"
        } else if code == "D" {
            "deleted"
        } else {
            "modified"
        };
        let previous = if kind == "renamed" {
            i += 1;
            Some(status[i - 1])
        } else {
            None
        };
        let path = status.get(i).copied().unwrap_or_default();
        i += 1;
        let mut file =
            json!({ "path": path, "kind": kind, "added": 0, "removed": 0, "binary": false });
        if let Some(p) = previous {
            file["previousPath"] = json!(p);
        }
        files.push(file);
    }
    let (mut i, mut n) = (0, 0);
    while i + 1 < numstat.len() && n < files.len() {
        let parts: Vec<&str> = numstat[i].split('\t').collect();
        i += 1;
        if parts.get(2) == Some(&"") {
            i += 2;
        }
        files[n]["binary"] = json!(parts.first() == Some(&"-"));
        files[n]["added"] = json!(
            parts
                .first()
                .and_then(|a| a.parse::<u64>().ok())
                .unwrap_or(0)
        );
        files[n]["removed"] = json!(
            parts
                .get(1)
                .and_then(|a| a.parse::<u64>().ok())
                .unwrap_or(0)
        );
        n += 1;
    }
    if !patches || files.is_empty() {
        return Ok(files);
    }
    let output = run(
        dir,
        &[
            "diff-tree",
            "-r",
            "-M",
            "-p",
            "--no-color",
            "--no-ext-diff",
            from,
            to,
        ],
    )
    .await?;
    if output.len() > PATCH_TOTAL * 4 {
        for file in files.iter_mut().filter(|f| f["binary"] != true) {
            file["truncated"] = json!(true);
        }
        return Ok(files);
    }
    let output = String::from_utf8_lossy(&output).into_owned();
    let mut chunks: Vec<&str> = Vec::new();
    let mut offset = 0;
    let starts: Vec<usize> = output
        .match_indices("diff --git ")
        .map(|(i, _)| i)
        .filter(|i| *i == 0 || output.as_bytes()[i - 1] == b'\n')
        .collect();
    for (k, start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(output.len());
        chunks.push(&output[start + 11..end]);
        offset = end;
    }
    let _ = offset;
    let mut total = 0;
    for (index, chunk) in chunks.iter().enumerate() {
        let Some(file) = files.get_mut(index) else {
            continue;
        };
        if file["binary"] == true {
            continue;
        }
        let Some(start) = chunk.find("\n@@") else {
            continue;
        };
        let patch = chunk[start + 1..]
            .strip_suffix('\n')
            .unwrap_or(&chunk[start + 1..]);
        let length = patch.encode_utf16().count();
        if length > PATCH_FILE
            || total + length > PATCH_TOTAL
            || patch.split('\n').count() > PATCH_LINES
        {
            file["truncated"] = json!(true);
            continue;
        }
        total += length;
        file["patch"] = json!(patch);
    }
    Ok(files)
}

async fn commit_detail(dir: &Path, value: &str) -> Result<Value> {
    let sha = commit_of(dir, value).await?;
    let format = format!(
        "--format=%H{UNIT}%P{UNIT}%an{UNIT}%ae{UNIT}%at{UNIT}%D{UNIT}%s{UNIT}%cn{UNIT}%ct{UNIT}%b"
    );
    let output = String::from_utf8_lossy(
        &run(
            dir,
            &[
                "log",
                "-1",
                "--decorate=short",
                "--decorate-refs=refs/heads/",
                "--decorate-refs=refs/tags/",
                &format,
                &sha,
            ],
        )
        .await?,
    )
    .into_owned();
    let f: Vec<&str> = output.split(UNIT).collect();
    let get = |i: usize| f.get(i).copied().unwrap_or_default();
    let parents: Vec<&str> = get(1).split(' ').filter(|p| !p.is_empty()).collect();
    let files = changes(
        dir,
        parents.first().copied().unwrap_or(EMPTY_TREE),
        &sha,
        true,
    )
    .await?;
    let body = if f.len() > 9 {
        f[9..].join(&UNIT.to_string())
    } else {
        String::new()
    };
    Ok(
        json!({ "sha": sha, "parents": parents, "author": get(2), "time": get(4).parse::<f64>().unwrap_or(0.0) * 1000.0, "subject": get(6), "source": source_of(get(3), get(6)), "refs": refs_of(get(5)),
        "added": files.iter().map(|f| f["added"].as_u64().unwrap_or(0)).sum::<u64>(), "removed": files.iter().map(|f| f["removed"].as_u64().unwrap_or(0)).sum::<u64>(),
        "body": body.trim_end_matches('\n'), "committer": get(7), "committedAt": get(8).parse::<f64>().unwrap_or(0.0) * 1000.0, "files": files }),
    )
}

async fn commits(dir: &Path, state: &GitState, params: &HashMap<String, String>) -> Result<Value> {
    if state.main.is_none() && state.refs.is_empty() {
        return Ok(json!({ "commits": [], "total": 0, "next": null, "authors": [] }));
    }
    let number = |key: &str, default: f64| {
        params
            .get(key)
            .map(|v| v.parse::<f64>().unwrap_or(f64::NAN))
            .unwrap_or(default)
    };
    let skip = number("skip", 0.0);
    let limit = number("limit", PAGE as f64).min(MAX_PAGE as f64);
    if !(skip.fract() == 0.0 && skip >= 0.0 && limit.fract() == 0.0 && limit >= 1.0) {
        return Err(AppError::invalid());
    }
    let (skip, limit) = (skip as usize, limit as usize);
    let author = params.get("author");
    if author
        .is_some_and(|a| a.encode_utf16().count() > 200 || a.contains('\0') || a.contains('\n'))
    {
        return Err(AppError::invalid());
    }
    let source = params.get("source");
    if source.is_some_and(|s| !["git", "dar", "agent"].contains(&s.as_str())) {
        return Err(AppError::invalid());
    }
    let revs: Vec<String> = match params.get("ref") {
        None => vec!["--all".into()],
        Some(r) if r == "all" => vec!["--all".into()],
        Some(r) => vec![ref_of(dir, Some(r), &BOTH).await?],
    };
    let filters: Vec<String> = author
        .map(|a| vec!["--fixed-strings".into(), format!("--author={a}")])
        .unwrap_or_default();
    let format = log_format();
    let base: Vec<String> = [
        "log",
        "--date-order",
        "--decorate=short",
        "--decorate-refs=refs/heads/",
        "--decorate-refs=refs/tags/",
        "--shortstat",
        "--diff-merges=first-parent",
        &format,
    ]
    .iter()
    .map(|s| s.to_string())
    .chain(filters.iter().cloned())
    .collect();
    fn as_refs(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }
    let (page, total) = if let Some(source) = source {
        // Where a commit came from is read from its author, which Git cannot exclude by: filter the IDs here.
        let ids = async |extra: &[&str]| -> Result<std::collections::HashSet<String>> {
            let mut args = vec!["rev-list"];
            args.extend_from_slice(extra);
            args.extend(revs.iter().map(String::as_str));
            Ok(lines(&run(dir, &args).await?).into_iter().collect())
        };
        let mut args = vec!["rev-list", "--date-order"];
        args.extend(filters.iter().map(String::as_str));
        args.extend(revs.iter().map(String::as_str));
        let ordered = lines(&run(dir, &args).await?);
        let dar = ids(&["--author=@dar\\.invalid>"]).await?;
        let agent = ids(&["--author=@dar\\.invalid>", "--grep=^Merge agent/"]).await?;
        let matching: Vec<String> = ordered
            .into_iter()
            .filter(|sha| match source.as_str() {
                "git" => !dar.contains(sha),
                "agent" => agent.contains(sha),
                _ => dar.contains(sha) && !agent.contains(sha),
            })
            .collect();
        let wanted: Vec<String> = matching.iter().skip(skip).take(limit).cloned().collect();
        let page = if wanted.is_empty() {
            Vec::new()
        } else {
            let mut args = as_refs(&base);
            args.extend(["--no-walk=unsorted", "--stdin"]);
            parse_log(&run_in(dir, &args, &format!("{}\n", wanted.join("\n"))).await?)
        };
        (page, matching.len())
    } else {
        let (s, m) = (format!("--skip={skip}"), format!("--max-count={limit}"));
        let mut args = as_refs(&base);
        args.extend([s.as_str(), m.as_str()]);
        args.extend(revs.iter().map(String::as_str));
        let page = parse_log(&run(dir, &args).await?);
        let mut count = vec!["rev-list", "--count"];
        count.extend(filters.iter().map(String::as_str));
        count.extend(revs.iter().map(String::as_str));
        (page, text(&run(dir, &count).await?).parse().unwrap_or(0))
    };
    let authors: Vec<String> = if skip > 0 {
        Vec::new()
    } else {
        let mut all: Vec<String> = Vec::new();
        for name in lines(&run(dir, &["log", "--all", "--max-count=5000", "--format=%an"]).await?) {
            if !all.contains(&name) {
                all.push(name);
            }
        }
        all.sort_by(|a, b| crate::http::locale_compare(a, b));
        all.truncate(100);
        all
    };
    let next = if skip + page.len() < total {
        json!(skip + page.len())
    } else {
        Value::Null
    };
    Ok(json!({ "commits": page, "total": total, "next": next, "authors": authors }))
}

async fn ahead_behind(dir: &Path, base: &str, head: &str) -> Result<(u64, u64)> {
    let out = text(
        &run(
            dir,
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("{base}...{head}"),
            ],
        )
        .await?,
    );
    let mut parts = out
        .split_whitespace()
        .map(|n| n.parse::<u64>().unwrap_or(0));
    let behind = parts.next().unwrap_or(0);
    Ok((parts.next().unwrap_or(0), behind))
}

async fn tags(dir: &Path) -> Result<Value> {
    let format = format!(
        "--format=%(refname:short){UNIT}%(objectname){UNIT}%(objecttype){UNIT}%(*objectname){UNIT}%(contents:subject){UNIT}%(taggername){UNIT}%(taggerdate:unix){UNIT}%(authorname){UNIT}%(authordate:unix)"
    );
    let rows = lines(
        &run(
            dir,
            &["for-each-ref", "--sort=-creatordate", &format, "refs/tags"],
        )
        .await?,
    );
    Ok(Value::Array(
        rows.iter()
            .map(|row| {
                let f: Vec<&str> = row.split(UNIT).collect();
                let get = |i: usize| f.get(i).copied().unwrap_or_default();
                let annotated = get(2) == "tag";
                json!({ "name": get(0), "sha": get(1), "target": if annotated { get(3) } else { get(1) }, "message": if annotated { get(4) } else { "" }, "author": if annotated { get(5) } else { get(7) },
                    "time": (if annotated { get(6) } else { get(8) }).parse::<f64>().unwrap_or(0.0) * 1000.0, "annotated": annotated })
            })
            .collect(),
    ))
}

/// Two refs or commit IDs; names label them. Pull requests also get each file's hunks.
async fn compare_tips(
    dir: &Path,
    base: &str,
    head: &str,
    base_name: &str,
    head_name: &str,
    patches: bool,
) -> Result<Value> {
    let (ahead, behind) = ahead_behind(dir, base, head).await?;
    let merge_base = plain(dir, &["merge-base", base, head]).await?.text();
    let merge_base = if merge_base.is_empty() {
        EMPTY_TREE.to_owned()
    } else {
        merge_base
    };
    let max = format!("--max-count={COMPARE}");
    let range = format!("{base}..{head}");
    let list = parse_log(
        &run(
            dir,
            &[
                "log",
                "--date-order",
                &max,
                "--shortstat",
                "--diff-merges=first-parent",
                &log_format(),
                &range,
            ],
        )
        .await?,
    );
    let tip = async |r: &str| -> Result<String> {
        Ok(text(&run(dir, &["rev-parse", "--verify", r]).await?))
    };
    // Whether Git can merge without conflicts; --quiet only answers, writing nothing.
    let mergeable = ahead > 0
        && (behind == 0
            || plain(dir, &["merge-tree", "--write-tree", "--quiet", base, head])
                .await?
                .code
                == 0);
    let files = if ahead > 0 {
        changes(dir, &merge_base, head, patches).await?
    } else {
        Vec::new()
    };
    Ok(
        json!({ "base": base_name, "head": head_name, "baseSha": tip(base).await?, "headSha": tip(head).await?, "ahead": list, "behind": behind, "fastForward": behind == 0 && ahead > 0, "mergeable": mergeable, "files": files }),
    )
}

fn download_name(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out
}

async fn archive(dir: &Path, reference: &str, name: &str) -> Result<Response> {
    let full = if is_sha(reference) {
        commit_of(dir, reference).await?
    } else {
        ref_of(dir, Some(reference), &BOTH).await?
    };
    let sha = text(
        &run(
            dir,
            &["rev-parse", "--verify", &format!("{full}^{{commit}}")],
        )
        .await?,
    );
    let base = download_name(name).trim_matches('-').to_owned();
    let label = format!(
        "{}-{}",
        if base.is_empty() { "repository" } else { &base },
        download_name_ref(reference)
    );
    let prefix = format!("--prefix={label}/");
    let mut child = git_command(
        dir,
        &[
            "--git-dir",
            path_str(dir),
            "archive",
            "--format=zip",
            &prefix,
            &sha,
        ],
        &[],
    )
    .stderr(std::process::Stdio::null())
    .spawn()
    .map_err(|_| AppError::internal())?;
    drop(child.stdin.take());
    let stdout = child.stdout.take().expect("stdout");
    let output = ReaderStream::new(stdout).chain(stream::once(async move {
        match child.wait().await {
            Ok(s) if s.success() => Ok(axum::body::Bytes::new()),
            _ => Err(std::io::Error::other("git archive failed")),
        }
    }));
    let mut response = Body::from_stream(output).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename*=UTF-8''{}.zip",
            encode_component(&label)
        ))
        .map_err(|_| AppError::internal())?,
    );
    Ok(response)
}

/// `ref.replace(/[^\w.-]+/g, "-")`.
fn download_name_ref(value: &str) -> String {
    download_name(value)
}

async fn blob(
    r: &Api<'_>,
    dir: &Path,
    reference: Option<&String>,
    path_value: Option<&String>,
) -> Result<Response> {
    let full = match reference {
        Some(v) if is_sha(v) => commit_of(dir, v).await?,
        other => {
            ref_of(
                dir,
                Some(other.map(String::as_str).unwrap_or("main")),
                &BOTH,
            )
            .await?
        }
    };
    let path = tree_path(path_value)?;
    if path.is_empty() {
        return Err(AppError::not_found());
    }
    let listed =
        String::from_utf8_lossy(&run(dir, &["ls-tree", "-z", "-l", &full, "--", &path]).await?)
            .into_owned();
    let first = listed.split('\0').next().unwrap_or_default();
    let meta: Vec<&str> = first
        .split('\t')
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let (mode, kind, sha, size) = (
        meta.first().copied().unwrap_or_default(),
        meta.get(1).copied().unwrap_or_default(),
        meta.get(2).copied().unwrap_or_default(),
        meta.get(3)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(u64::MAX),
    );
    if kind != "blob" || size > 256 * 1024 * 1024 {
        return Err(AppError::not_found());
    }
    let content = run(dir, &["cat-file", "blob", sha]).await?;
    let is_text = mode != "120000"
        && content.len() <= BLOB
        && !content[..content.len().min(8000)].contains(&0);
    let name = path.rsplit('/').next().unwrap_or(&path);
    let inline = is_text && r.query.contains_key("view");
    let length = content.len();
    let body = if *r.method == Method::HEAD {
        Vec::new()
    } else {
        content
    };
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(if is_text {
            "text/plain; charset=utf-8"
        } else {
            "application/octet-stream"
        }),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "{}; filename*=UTF-8''{}",
            if inline { "inline" } else { "attachment" },
            encode_component(name)
        ))
        .map_err(|_| AppError::internal())?,
    );
    Ok(response)
}

impl Git<'_> {
    async fn branches(&self, folder_id: &str, dir: &Path) -> Result<Value> {
        let format = format!(
            "--format=%(refname:short){UNIT}%(objectname){UNIT}%(contents:subject){UNIT}%(authorname){UNIT}%(authordate:unix)"
        );
        let rows = lines(&run(dir, &["for-each-ref", &format, "refs/heads"]).await?);
        let all: Vec<Value> = rows
            .iter()
            .map(|row| {
                let f: Vec<&str> = row.split(UNIT).collect();
                let get = |i: usize| f.get(i).copied().unwrap_or_default();
                json!({ "name": get(0), "sha": get(1), "subject": get(2), "author": get(3), "time": get(4).parse::<f64>().unwrap_or(0.0) * 1000.0, "ahead": 0, "behind": 0 })
            })
            .collect();
        let main = all.iter().find(|b| b["name"] == "main").cloned();
        let mut list = Vec::new();
        for branch in all.into_iter().filter(|b| b["name"] != "main") {
            let mut branch = branch;
            if main.is_some() {
                let (ahead, behind) = ahead_behind(
                    dir,
                    MAIN_REF,
                    &format!("refs/heads/{}", branch["name"].as_str().unwrap_or_default()),
                )
                .await?;
                branch["ahead"] = json!(ahead);
                branch["behind"] = json!(behind);
            }
            list.push(branch);
        }
        list.sort_by(|a, b| {
            b["time"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&a["time"].as_f64().unwrap_or(0.0))
        });
        let history = self.overview(folder_id).await?;
        let agent: Vec<Value> = history["branches"]
            .as_array()
            .map(|b| b.iter().map(|b| json!({ "name": b["name"], "threadId": b["threadId"], "message": b["message"], "author": b["author"], "modifiedAt": b["modifiedAt"], "files": b["changes"].as_array().map_or(0, Vec::len) })).collect())
            .unwrap_or_default();
        Ok(json!({ "main": main, "branches": list, "agent": agent }))
    }

    /// Merges one branch into another from the web: a fast-forward, or a merge commit Git writes
    /// without a working tree (refused on conflicts), applied like a push.
    async fn merge(&self, folder_id: &str, input: &Map<String, Value>) -> Result<Value> {
        if input.keys().any(|k| {
            ![
                "base",
                "head",
                "baseSha",
                "headSha",
                "mode",
                "message",
                "deleteBranch",
            ]
            .contains(&k.as_str())
        }) {
            return Err(AppError::invalid());
        }
        let s = |k: &str| input.get(k).and_then(Value::as_str);
        let (Some(base), Some(head), Some(base_sha), Some(head_sha), Some(mode)) =
            (s("base"), s("head"), s("baseSha"), s("headSha"), s("mode"))
        else {
            return Err(AppError::invalid());
        };
        if base == head || (mode != "fast-forward" && mode != "merge") {
            return Err(AppError::invalid());
        }
        let message = match input.get("message") {
            None => None,
            Some(Value::String(m)) if m.encode_utf16().count() <= 2000 && !m.contains('\0') => {
                Some(m.clone())
            }
            Some(_) => return Err(AppError::invalid()),
        };
        let delete = match input.get("deleteBranch") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(AppError::invalid()),
        };
        if head == "main" && delete {
            return Err(AppError::with("forbidden", 403, "main cannot be deleted."));
        }
        let repos = &self.r.runtime.git;
        let _guard = repos.lock(folder_id).await;
        let (dir, _) = repos.sync_locked(self.session(), folder_id).await?;
        let base_ref = ref_of(&dir, Some(base), &["heads"]).await?;
        let head_ref = ref_of(&dir, Some(head), &["heads"]).await?;
        let current_base = text(&run(&dir, &["rev-parse", "--verify", &base_ref]).await?);
        let current_head = text(&run(&dir, &["rev-parse", "--verify", &head_ref]).await?);
        if current_base != base_sha || current_head != head_sha {
            return Err(AppError::with(
                "conflict",
                409,
                "The branches changed since you opened them. Review them again.",
            ));
        }
        let (ahead, behind) = ahead_behind(&dir, &base_ref, &head_ref).await?;
        if ahead == 0 {
            return Err(AppError::with(
                "nothingToMerge",
                409,
                &format!("{base} already has every commit on {head}."),
            ));
        }
        let mut target = current_head.clone();
        if mode == "fast-forward" {
            if behind > 0 {
                return Err(AppError::with(
                    "conflict",
                    409,
                    &format!(
                        "{base} has commits {head} does not, so it cannot fast-forward. Create a merge commit instead."
                    ),
                ));
            }
        } else {
            let merged = plain(
                &dir,
                &[
                    "merge-tree",
                    "--write-tree",
                    "-z",
                    "--name-only",
                    &current_base,
                    &current_head,
                ],
            )
            .await?;
            let output = String::from_utf8_lossy(&merged.stdout).into_owned();
            let fields: Vec<&str> = output.split('\0').collect();
            if merged.code == 1 {
                let end = fields
                    .iter()
                    .skip(1)
                    .position(|f| f.is_empty())
                    .map(|p| p + 1)
                    .unwrap_or(fields.len());
                let files: Vec<&str> = fields[1..end]
                    .iter()
                    .copied()
                    .filter(|f| !f.is_empty())
                    .collect();
                let shown = files.iter().take(8).copied().collect::<Vec<_>>().join(", ");
                let more = if files.len() > 8 {
                    format!(" and {} more", files.len() - 8)
                } else {
                    String::new()
                };
                let lead = if files.len() == 1 {
                    "This file conflicts"
                } else {
                    "These files conflict"
                };
                return Err(AppError::with(
                    "mergeConflict",
                    409,
                    &format!("{lead}: {shown}{more}. Merge with Git, resolve them and push."),
                ));
            }
            if merged.code != 0 || !fields.first().is_some_and(|f| is_sha(f)) {
                return Err(AppError::with(
                    "gitFailed",
                    502,
                    "Git could not merge these branches.",
                ));
            }
            let user = self.r.user();
            let username = self.r.username();
            let display: String = user["displayName"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .filter(|c| !"<>\n\r\0".contains(*c))
                .collect::<String>()
                .trim()
                .to_owned();
            let name = if display.is_empty() {
                username.to_owned()
            } else {
                display
            };
            let email = format!("{username}@dar.invalid");
            let default = format!(
                "Merge branch '{head}'{}",
                if base == "main" {
                    String::new()
                } else {
                    format!(" into {base}")
                }
            );
            let text_message = format!(
                "{}\n",
                message
                    .as_deref()
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .unwrap_or(&default)
            );
            let env = [
                ("GIT_AUTHOR_NAME", name.as_str()),
                ("GIT_AUTHOR_EMAIL", email.as_str()),
                ("GIT_COMMITTER_NAME", name.as_str()),
                ("GIT_COMMITTER_EMAIL", email.as_str()),
            ];
            let out = git_full(
                &dir,
                &[
                    "--git-dir",
                    path_str(&dir),
                    "commit-tree",
                    fields[0],
                    "-p",
                    &current_base,
                    "-p",
                    &current_head,
                    "-F",
                    "-",
                ],
                &env,
                text_message.as_bytes(),
                false,
                std::time::Duration::from_secs(300),
            )
            .await?;
            target = out.text();
        }
        let mut commands = vec![RefCommand {
            old: current_base.clone(),
            new: target.clone(),
            name: base_ref.clone(),
        }];
        if delete {
            commands.push(RefCommand {
                old: current_head.clone(),
                new: ZERO.into(),
                name: head_ref.clone(),
            });
        }
        let results =
            push_from_dar(self.r.runtime, self.session(), folder_id, &dir, &commands).await?;
        let error = results
            .get(&base_ref)
            .cloned()
            .flatten()
            .or_else(|| results.get(&head_ref).cloned().flatten());
        if let Some(error) = error {
            let message = if error.contains("uncommitted") || error.contains("not committed") {
                let mut c = error.chars();
                format!(
                    "{}{}.",
                    c.next()
                        .map(|f| f.to_uppercase().to_string())
                        .unwrap_or_default(),
                    c.as_str()
                )
            } else {
                format!("The merge was refused: {error}.")
            };
            return Err(AppError::with("conflict", 409, &message));
        }
        Ok(json!({ "sha": target, "fastForward": target == current_head, "deleted": delete }))
    }

    async fn delete_ref(&self, folder_id: &str, input: &Map<String, Value>) -> Result<Value> {
        let name = input.get("name").and_then(Value::as_str);
        let old = input.get("old").and_then(Value::as_str);
        let short = name.and_then(|n| {
            n.strip_prefix("refs/heads/")
                .or_else(|| n.strip_prefix("refs/tags/"))
        });
        let (Some(name), Some(old), Some(short)) = (name, old, short) else {
            return Err(AppError::invalid());
        };
        if !valid_ref_name(short) || !is_sha(old) {
            return Err(AppError::invalid());
        }
        if name == MAIN_REF {
            return Err(AppError::with("forbidden", 403, "main cannot be deleted."));
        }
        let repos = &self.r.runtime.git;
        let _guard = repos.lock(folder_id).await;
        let outcome = repos.call(self.session(), "push", json!({ "folderId": folder_id, "updates": [{ "name": name, "old": old, "new": ZERO }], "atomic": true, "commits": [], "tags": [], "blobs": [] })).await?;
        let error = outcome["results"].as_array().and_then(|r| {
            r.iter()
                .find_map(|x| x["error"].as_str().map(str::to_owned))
        });
        if let Some(error) = error {
            let message = if error == "fetch first" {
                "It changed since you opened it. Reload and try again.".to_owned()
            } else {
                format!("It could not be deleted: {error}.")
            };
            return Err(AppError::with("conflict", 409, &message));
        }
        repos.sync_locked(self.session(), folder_id).await?;
        Ok(json!({ "deleted": name }))
    }

    /// A pull request's commits and files (open: the branches now; merged or closed: its tips).
    async fn pull_changes(&self, folder_id: &str, pull: &Value) -> Result<Value> {
        if pull["agent"] == true {
            if pull["state"] != "open" {
                return Ok(
                    json!({ "note": if pull["state"] == "merged" { "These changes were merged into the folder." } else { "This pull request is closed." } }),
                );
            }
            let history = self.overview(folder_id).await?;
            let branch = history["branches"]
                .as_array()
                .and_then(|b| b.iter().find(|b| b["name"] == pull["head"]).cloned());
            let (Some(branch), false) = (branch, history["repo"].is_null()) else {
                return Ok(json!({ "note": "The Agent branch no longer exists." }));
            };
            return Ok(
                json!({ "agent": { "changes": branch["changes"], "revision": history["repo"]["revision"], "threadId": branch["threadId"] } }),
            );
        }
        let repos = &self.r.runtime.git;
        let _guard = repos.lock(folder_id).await;
        let (dir, _) = repos.sync_locked(self.session(), folder_id).await?;
        let (base, head) = (
            pull["base"].as_str().unwrap_or_default(),
            pull["head"].as_str().unwrap_or_default(),
        );
        if pull["state"] == "open" {
            let tips = async {
                Ok::<_, AppError>((
                    ref_of(&dir, Some(base), &["heads"]).await?,
                    ref_of(&dir, Some(head), &["heads"]).await?,
                ))
            }
            .await;
            return match tips {
                Ok((b, h)) => {
                    Ok(json!({ "compare": compare_tips(&dir, &b, &h, base, head, true).await? }))
                }
                Err(e) if e.code == "notFound" => Ok(
                    json!({ "note": format!("{head} or {base} no longer exists. Push the branch again to continue.") }),
                ),
                Err(e) => Err(e),
            };
        }
        let have = async |sha: Option<&str>| match sha.filter(|s| !s.is_empty()) {
            Some(s) => commit_of(&dir, s).await.is_ok(),
            None => false,
        };
        let (base_sha, head_sha) = (pull["baseSha"].as_str(), pull["headSha"].as_str());
        if have(base_sha).await && have(head_sha).await {
            return Ok(
                json!({ "compare": compare_tips(&dir, base_sha.unwrap_or_default(), head_sha.unwrap_or_default(), base, head, true).await? }),
            );
        }
        Ok(
            json!({ "note": "Its commits are no longer in the repository; the branch was deleted without being merged." }),
        )
    }

    /// /issues, /labels and /comments of a repository; the database checks access and input.
    async fn tracker_route(
        &self,
        folder_id: &str,
        path: &str,
        body: Body,
    ) -> Result<Option<(Value, StatusCode)>> {
        let method = self.r.method.clone();
        let call = async |operation: &str, mut document: Map<String, Value>| {
            document.insert("folderId".into(), json!(folder_id));
            self.tracker(operation, Value::Object(document)).await
        };
        let input = async |body: Body, extra: Map<String, Value>| -> Result<Map<String, Value>> {
            let mut p = self.r.body(body, 256 * 1024).await?;
            p.extend(extra);
            Ok(p)
        };
        let one = |k: &str, v: Value| {
            let mut m = Map::new();
            m.insert(k.into(), v);
            m
        };
        let ok = |v: Value| Ok(Some((v, StatusCode::OK)));
        let uuid = |v: &str| {
            v.len() == 36
                && v.bytes()
                    .all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-'))
        };
        if path == "/issues" && method == Method::GET {
            let mut document = Map::new();
            document.insert(
                "kind".into(),
                self.r
                    .q_opt("kind")
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            document.insert(
                "state".into(),
                json!(self.r.q_opt("state").unwrap_or_else(|| "open".into())),
            );
            if let Some(label) = self.r.q_opt("label").filter(|l| !l.is_empty()) {
                document.insert("label".into(), json!(label));
            }
            // A missing kind is not sent, as JSON.stringify leaves out undefined.
            if document["kind"].is_null() {
                document.remove("kind");
            }
            return ok(call("list", document).await?);
        }
        if path == "/issues" && method == Method::POST {
            return Ok(Some((
                call("create", input(body, Map::new()).await?).await?,
                StatusCode::CREATED,
            )));
        }
        if path == "/labels" && method == Method::GET {
            return ok(call("labels", Map::new()).await?);
        }
        if path == "/labels" && method == Method::POST {
            return Ok(Some((
                call("saveLabel", input(body, Map::new()).await?).await?,
                StatusCode::CREATED,
            )));
        }
        if let Some(id) = path.strip_prefix("/labels/").filter(|i| uuid(i)) {
            if method == Method::PATCH {
                return ok(call("saveLabel", input(body, one("id", json!(id))).await?).await?);
            }
            if method == Method::DELETE {
                return ok(call("deleteLabel", one("id", json!(id))).await?);
            }
        }
        if let Some(id) = path.strip_prefix("/comments/").filter(|i| uuid(i)) {
            if method == Method::PATCH {
                return ok(call("editComment", input(body, one("id", json!(id))).await?).await?);
            }
            if method == Method::DELETE {
                return ok(call("deleteComment", one("id", json!(id))).await?);
            }
        }
        let Some(rest) = path.strip_prefix("/issues/") else {
            return Ok(None);
        };
        let (digits, tail) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        if digits.is_empty()
            || digits.len() > 6
            || digits.starts_with('0')
            || !digits.bytes().all(|b| b.is_ascii_digit())
            || !["", "/comments", "/changes"].contains(&tail)
        {
            return Ok(None);
        }
        let number = json!(digits.parse::<u64>().unwrap_or(0));
        match (tail, method) {
            ("", Method::GET) => ok(call("issue", one("number", number)).await?),
            ("", Method::PATCH) => {
                ok(call("update", input(body, one("number", number)).await?).await?)
            }
            ("/comments", Method::POST) => Ok(Some((
                call("comment", input(body, one("number", number)).await?).await?,
                StatusCode::CREATED,
            ))),
            ("/changes", Method::GET) => {
                let pull = call("issue", one("number", number)).await?;
                if pull["kind"] != "pull" {
                    return Err(AppError::not_found());
                }
                ok(self.pull_changes(folder_id, &pull).await?)
            }
            _ => Ok(None),
        }
    }
}

pub async fn handle(r: &Api<'_>, body: Body) -> Result<Response> {
    let g = Git { r };
    let path = r.path;
    let method = r.method;
    let send = |v: Value| Ok(json_response(&v, StatusCode::OK));
    if path == "/api/v1/git/repositories" && *method == Method::GET {
        return send(g.repositories().await?);
    }
    if path == "/api/v1/git/activity" && *method == Method::GET {
        return send(g.activity().await?);
    }
    let rest = path
        .strip_prefix("/api/v1/git/repositories/")
        .ok_or_else(AppError::not_found)?;
    if rest.len() < 36
        || !rest[..36]
            .bytes()
            .all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-'))
    {
        return Err(AppError::not_found());
    }
    let (folder_id, tail) = rest.split_at(36);
    if ["/issues", "/labels", "/comments"]
        .iter()
        .any(|p| tail == *p || tail.starts_with(&format!("{p}/")))
    {
        return match g.tracker_route(folder_id, tail, body).await? {
            Some((value, status)) => Ok(json_response(&value, status)),
            None => Err(AppError::not_found()),
        };
    }
    // (/[a-z]+)?(/<sha>)?
    let (segment, sha) = match tail.strip_prefix('/') {
        None if tail.is_empty() => ("", None),
        Some(t) => {
            let (word, after) = t.split_at(t.find('/').unwrap_or(t.len()));
            if word.is_empty() || !word.bytes().all(|b| b.is_ascii_lowercase()) {
                return Err(AppError::not_found());
            }
            match after.strip_prefix('/') {
                None if after.is_empty() => (word, None),
                Some(s) if is_sha(s) => (word, Some(s.to_owned())),
                _ => return Err(AppError::not_found()),
            }
        }
        None => return Err(AppError::not_found()),
    };
    if segment.is_empty() && *method == Method::GET {
        let row = g.row_of(folder_id).await?;
        return send(g.summary(&row).await);
    }
    if segment == "refs" && *method == Method::DELETE {
        let input = r.body(body, 256 * 1024).await?;
        return send(g.delete_ref(folder_id, &input).await?);
    }
    if segment == "merge" && *method == Method::POST {
        let input = r.body(body, 256 * 1024).await?;
        return send(g.merge(folder_id, &input).await?);
    }
    if *method != Method::GET && !(*method == Method::HEAD && segment == "blob") {
        return Err(AppError::not_found());
    }
    let repos = &r.runtime.git;
    if segment == "archive" {
        let name = g.row_of(folder_id).await?["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let _guard = repos.lock(folder_id).await;
        let (dir, _) = repos.sync_locked(g.session(), folder_id).await?;
        return archive(
            &dir,
            &r.q_opt("ref").unwrap_or_else(|| "main".into()),
            &name,
        )
        .await;
    }
    let _guard = repos.lock(folder_id).await;
    let (dir, state) = repos.sync_locked(g.session(), folder_id).await?;
    if segment == "blob" {
        return blob(r, &dir, r.query.get("ref"), r.query.get("path")).await;
    }
    let value = match (segment, sha) {
        ("tree", _) => tree(&dir, &state, r.query.get("ref"), r.query.get("path")).await?,
        ("commits", Some(sha)) => {
            let mut detail = commit_detail(&dir, &sha).await?;
            let pin = repos
                .call(
                    g.session(),
                    "changesetOf",
                    json!({ "folderId": folder_id, "sha": detail["sha"] }),
                )
                .await?;
            if pin["changesetId"].is_string() {
                detail["restore"] =
                    json!({ "changesetId": pin["changesetId"], "revision": pin["revision"] });
            }
            detail
        }
        ("commits", None) => commits(&dir, &state, r.query).await?,
        ("branches", _) => g.branches(folder_id, &dir).await?,
        ("tags", _) => tags(&dir).await?,
        ("compare", _) => {
            let (b, h) = (r.q_opt("base"), r.q_opt("head"));
            let base = ref_of(&dir, b.as_deref(), &BOTH).await?;
            let head = ref_of(&dir, h.as_deref(), &BOTH).await?;
            compare_tips(
                &dir,
                &base,
                &head,
                b.as_deref().unwrap_or_default(),
                h.as_deref().unwrap_or_default(),
                false,
            )
            .await?
        }
        _ => return Err(AppError::not_found()),
    };
    send(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readme_descriptions() {
        assert_eq!(
            describe(
                "# Title\n\n[![CI](x.svg)](ci) ![logo](l.png)\nA *small* [tool](https://x) for `git`.\nSecond line.\n\nMore."
            ),
            "A small tool for git. Second line."
        );
        assert!(
            valid_ref_name("feature/one")
                && !valid_ref_name("a..b")
                && !valid_ref_name(".hidden")
                && !valid_ref_name("HEAD")
        );
        assert_eq!(
            git_alias("amina", "Projects/My site"),
            "/dav/git/amina/Projects/My%20site.git"
        );
    }
}
