//! Issues and pull requests (`tracker.ts`, docs/git.md#issues-and-pull-requests). Anyone who can
//! read the folder opens issues and comments; people who can write also label, open, close and
//! merge pull requests. Git pushes call `pulls_after_push` in the same transaction.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Map, Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::files::Role;
use crate::history::{HistoryFiles, folder_of, keys};
use crate::tables::*;
use crate::{Res, fail, now};

type Doc = Map<String, Value>;

const TITLE: usize = 256;
const BODY: usize = 65_536;
const LABEL_NAME: usize = 50;
const LABEL_DESCRIPTION: usize = 200;
const LABELS: usize = 100;
const LABELS_PER_ITEM: usize = 10;
const ITEMS: usize = 10_000;
const COMMENTS: usize = 2_500;
const PAGE: usize = 500;
const COLORS: [&str; 9] = ["plum", "violet", "blue", "teal", "green", "amber", "orange", "rose", "gray"];

fn issue_id(repo_id: &str, number: u32) -> String {
    format!("{repo_id}/{number}")
}
fn writer(role: Role) -> bool {
    role != Role::Read
}
fn agent_head(name: &str) -> bool {
    name.starts_with("agent/")
}
fn utf16(s: &str) -> usize {
    s.encode_utf16().count()
}
fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}
/// `value.replace(/\s+/g, " ").trim()`.
fn squash(value: &str) -> String {
    value.split(js_space).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
}
fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `#n` mentions (`issueReference` in shared/git.ts).
pub fn issue_references(text: &str) -> Vec<u32> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    for i in 0..chars.len() {
        if chars[i] != '#' || i > 0 && (word(chars[i - 1]) || matches!(chars[i - 1], '&' | '/' | '#')) {
            continue;
        }
        if let Some(n) = digits(&chars, i + 1) {
            out.push(n);
        }
    }
    out
}

/// One to six digits at `start` followed by a word boundary.
fn digits(chars: &[char], start: usize) -> Option<u32> {
    let mut end = start;
    while end < chars.len() && chars[end].is_ascii_digit() {
        end += 1;
    }
    if end == start || end - start > 6 || end < chars.len() && word(chars[end]) {
        return None;
    }
    chars[start..end].iter().collect::<String>().parse().ok()
}

/// "Closes #n" and its kin (`closingReference` in shared/git.ts).
pub fn closing_references(text: &str) -> Vec<u32> {
    let chars: Vec<char> = text.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut out = Vec::new();
    const WORDS: [&str; 9] = ["closes", "closed", "close", "fixes", "fixed", "fix", "resolves", "resolved", "resolve"];
    for i in 0..chars.len() {
        if i > 0 && word(chars[i - 1]) || i < chars.len() && !word(chars[i]) {
            continue;
        }
        for w in WORDS {
            let w: Vec<char> = w.chars().collect();
            if chars.len() < i + w.len() || chars[i..i + w.len()] != w[..] {
                continue;
            }
            let mut j = i + w.len();
            let before = j;
            while j < chars.len() && js_space(chars[j]) {
                j += 1;
            }
            let mut space = j > before;
            if j < chars.len() && chars[j] == ':' {
                j += 1;
                let after = j;
                while j < chars.len() && js_space(chars[j]) {
                    j += 1;
                }
                space = j > after;
            }
            if space && j < chars.len() && chars[j] == '#'
                && let Some(n) = digits(&chars, j + 1)
            {
                out.push(n);
                break;
            }
        }
    }
    out
}

fn repo_for(ctx: &ReducerContext, files: &HistoryFiles, p: &Doc) -> Res<(FileRepo, Role)> {
    let (root, role) = folder_of(ctx, files, p.get("folderId"), false)?;
    match ctx.db.file_repos().id().find(root.id) {
        Some(repo) => Ok((repo, role)),
        None => fail("notFound"),
    }
}

fn issue_of(ctx: &ReducerContext, repo: &FileRepo, value: Option<&Value>) -> Res<GitIssue> {
    let number = match value.and_then(Value::as_f64) {
        Some(n) if n.fract() == 0.0 && n >= 1.0 && n <= u32::MAX as f64 => n as u32,
        _ => return fail("invalidInput"),
    };
    ctx.db.git_issues().id().find(issue_id(&repo.id, number)).ok_or_else(|| "notFound".to_owned())
}

fn title(value: Option<&Value>) -> Res<String> {
    let text = value.and_then(Value::as_str).map(squash).unwrap_or_default();
    if text.is_empty() || utf16(&text) > TITLE { fail("invalidInput") } else { Ok(text) }
}

fn body(value: Option<&Value>) -> Res<String> {
    let Some(value) = value else { return Ok(String::new()) };
    match value.as_str() {
        Some(s) if utf16(s) <= BODY && !s.contains('\0') => Ok(s.replace("\r\n", "\n").trim_end_matches(js_space).to_owned()),
        _ => fail("invalidInput"),
    }
}

fn branch_name(value: Option<&Value>) -> Res<String> {
    let Some(s) = value.and_then(Value::as_str) else { return fail("invalidInput") };
    let bad = s.is_empty()
        || utf16(s) > 200
        || s.chars().any(|c| (c as u32) <= 0x20 || c as u32 == 0x7f || "~^:?*[\\".contains(c))
        || s.contains("..")
        || s.contains("@{")
        || s.contains("//")
        || s.starts_with('/')
        || s.ends_with('/')
        || s.ends_with('.')
        || s.split('/').any(|part| part.ends_with(".lock"));
    if bad { fail("invalidInput") } else { Ok(s.to_owned()) }
}

fn person(ctx: &ReducerContext, user_id: &str) -> String {
    let account = ctx.db.accounts().id().find(user_id.to_owned());
    account.map(|a| if a.display_name.is_empty() { a.username } else { a.display_name }).filter(|n| !n.is_empty()).unwrap_or_else(|| "Removed account".into())
}

fn label_rows(ctx: &ReducerContext, repo_id: &str) -> Vec<GitLabel> {
    ctx.db.git_labels().repo_id().filter(repo_id).collect()
}
fn label_view(row: &GitLabel) -> Value {
    json!({ "id": row.id, "name": row.name, "color": row.color, "description": row.description })
}
fn sorted_labels(ctx: &ReducerContext, repo_id: &str) -> Value {
    let mut rows = label_rows(ctx, repo_id);
    rows.sort_by(|a, b| crate::locale_compare(&a.name, &b.name));
    Value::Array(rows.iter().map(label_view).collect())
}
fn label_list(value: &str) -> Vec<String> {
    value.split(' ').filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// A branch's tip as the database has it: main follows the folder's history; agent branches have none.
fn tip_of(ctx: &ReducerContext, repo: &FileRepo, name: &str) -> Option<String> {
    if name == "main" {
        if repo.head_id.is_empty() {
            return None;
        }
        // main with commits Git has not seen yet has no Git ID until the next fetch; it still exists.
        return Some(ctx.db.file_git_objects().changeset_id().filter(repo.head_id.as_str()).next().map(|p| p.sha).unwrap_or_default());
    }
    ctx.db.file_git_refs().id().find(format!("{}/refs/heads/{name}", repo.id)).map(|r| r.target)
}

fn branch_exists(ctx: &ReducerContext, repo: &FileRepo, name: &str) -> bool {
    if agent_head(name) {
        return ctx.db.file_branches().repo_id().filter(repo.id.as_str()).any(|b| b.name == name);
    }
    tip_of(ctx, repo, name).is_some()
}

fn touch(ctx: &ReducerContext, row: GitIssue, at: f64) -> GitIssue {
    let next = GitIssue { updated_at: at, revision: row.revision + 1, ..row };
    ctx.db.git_issues().id().update(next.clone());
    next
}

fn next_sequence(ctx: &ReducerContext, id: &str) -> u32 {
    ctx.db.git_comments().issue_id().filter(id).map(|c| c.sequence).max().unwrap_or(0) + 1
}

fn add_event(ctx: &ReducerContext, files: &HistoryFiles, row: &GitIssue, event: &str, author_id: &str, at: f64, data: Value) {
    let data = if data.is_null() { json!({}) } else { data };
    ctx.db.git_comments().insert(GitComment {
        id: files.uuid(ctx),
        issue_id: row.id.clone(),
        repo_id: row.repo_id.clone(),
        sequence: next_sequence(ctx, &row.id),
        kind: event.into(),
        author_id: author_id.into(),
        body: String::new(),
        data: crate::stringify(&data),
        created_at: at,
        edited_at: 0.0,
    });
}

/// Records "referenced from #n" on each item a text mentions, once per pair.
fn mention(ctx: &ReducerContext, files: &HistoryFiles, row: &GitIssue, text: &str, author_id: &str, at: f64) {
    let mut numbers: Vec<u32> = Vec::new();
    for n in issue_references(text) {
        if n != row.number && !numbers.contains(&n) {
            numbers.push(n);
        }
    }
    for number in numbers.into_iter().take(50) {
        let Some(target) = ctx.db.git_issues().id().find(issue_id(&row.repo_id, number)) else { continue };
        let seen = ctx.db.git_comments().issue_id().filter(target.id.as_str()).any(|item| {
            item.kind == "referenced" && serde_json::from_str::<Value>(&item.data).ok().and_then(|d| d.get("number").and_then(Value::as_f64)) == Some(row.number as f64)
        });
        if !seen {
            add_event(ctx, files, &target, "referenced", author_id, at, json!({ "number": row.number }));
            touch(ctx, target, at);
        }
    }
}

/// Closes the open issues a merged pull request's description names with "Closes #n".
fn close_named(ctx: &ReducerContext, files: &HistoryFiles, pull: &GitIssue, user_id: &str, at: f64) {
    let numbers: BTreeSet<u32> = closing_references(&pull.body).into_iter().collect();
    let mut order: Vec<u32> = Vec::new();
    for n in closing_references(&pull.body) {
        if !order.contains(&n) {
            order.push(n);
        }
    }
    debug_assert_eq!(numbers.len(), order.len());
    for number in order {
        let Some(target) = ctx.db.git_issues().id().find(issue_id(&pull.repo_id, number)) else { continue };
        if target.kind != "issue" || target.state != "open" {
            continue;
        }
        let target = touch(ctx, GitIssue { state: "closed".into(), closed_at: at, closed_by: user_id.into(), ..target }, at);
        add_event(ctx, files, &target, "closed", user_id, at, json!({ "number": pull.number }));
    }
}

fn merged(ctx: &ReducerContext, files: &HistoryFiles, pull: GitIssue, user_id: &str, at: f64, base_sha: &str, head_sha: &str, merged_sha: &str) {
    let row = touch(ctx, GitIssue { state: "merged".into(), closed_at: at, closed_by: user_id.into(), base_sha: base_sha.into(), head_sha: head_sha.into(), merged_sha: merged_sha.into(), ..pull }, at);
    add_event(ctx, files, &row, "merged", user_id, at, if merged_sha.is_empty() { json!({}) } else { json!({ "sha": merged_sha }) });
    close_named(ctx, files, &row, user_id, at);
}

fn summary(ctx: &ReducerContext, row: &GitIssue) -> Value {
    let comments = ctx.db.git_comments().issue_id().filter(row.id.as_str()).filter(|c| c.kind == "comment").count();
    let mut v = json!({ "number": row.number, "kind": row.kind, "title": row.title, "state": row.state, "author": person(ctx, &row.author_id), "createdAt": row.created_at, "updatedAt": row.updated_at,
        "comments": comments, "labels": label_list(&row.labels) });
    if row.closed_at != 0.0 {
        v["closedAt"] = json!(row.closed_at);
    }
    if row.kind == "pull" {
        v["base"] = json!(row.base);
        v["head"] = json!(row.head);
        v["agent"] = json!(agent_head(&row.head));
    }
    v
}

fn detail(ctx: &ReducerContext, repo: &FileRepo, role: Role, user_id: &str, row: &GitIssue) -> Value {
    let mut items: Vec<GitComment> = ctx.db.git_comments().issue_id().filter(row.id.as_str()).collect();
    items.sort_by_key(|c| c.sequence);
    let timeline: Vec<Value> = items
        .iter()
        .map(|item| {
            if item.kind == "comment" {
                let mut v = json!({ "id": item.id, "kind": "comment", "author": person(ctx, &item.author_id), "body": item.body, "createdAt": item.created_at,
                    "canEdit": item.author_id == user_id, "canDelete": item.author_id == user_id || writer(role) });
                if item.edited_at != 0.0 {
                    v["editedAt"] = json!(item.edited_at);
                }
                return v;
            }
            let mut v = json!({ "id": item.id, "kind": "event", "event": item.kind, "author": person(ctx, &item.author_id), "createdAt": item.created_at });
            let data: Value = serde_json::from_str(&item.data).unwrap_or(json!({}));
            if let Value::Object(data) = &data {
                for (k, value) in data {
                    v[k] = value.clone();
                }
            }
            let other = data.get("number").and_then(Value::as_f64).filter(|n| *n != 0.0).and_then(|n| ctx.db.git_issues().id().find(issue_id(&repo.id, n as u32)));
            if let Some(other) = other {
                v["title"] = json!(other.title);
            }
            v
        })
        .collect();
    let own = row.author_id == user_id;
    let open = row.state == "open";
    let mut v = summary(ctx, row);
    v["body"] = json!(row.body);
    v["revision"] = json!(row.revision);
    if !row.closed_by.is_empty() {
        v["closedBy"] = json!(person(ctx, &row.closed_by));
    }
    v["timeline"] = Value::Array(timeline);
    for (key, value) in [("baseSha", &row.base_sha), ("headSha", &row.head_sha), ("mergedSha", &row.merged_sha)] {
        if !value.is_empty() {
            v[key] = json!(value);
        }
    }
    let close = row.state != "merged" && if row.kind == "pull" { writer(role) } else { own || writer(role) };
    v["permissions"] = json!({ "edit": own || writer(role), "close": close, "label": writer(role), "merge": row.kind == "pull" && open && writer(role), "comment": true });
    v
}

fn chosen_labels(ctx: &ReducerContext, repo: &FileRepo, value: Option<&Value>) -> Res<Vec<String>> {
    let Some(list) = value.and_then(Value::as_array).filter(|a| a.len() <= LABELS_PER_ITEM) else { return fail("invalidInput") };
    let known: Vec<String> = label_rows(ctx, &repo.id).into_iter().map(|r| r.id).collect();
    let mut out: Vec<String> = Vec::new();
    for id in list {
        let Some(id) = id.as_str().filter(|id| known.iter().any(|k| k == id)) else { return fail("invalidInput") };
        if !out.iter().any(|o| o == id) {
            out.push(id.to_owned());
        }
    }
    Ok(out)
}

/// An open pull request from head into base may exist only once.
fn duplicate(ctx: &ReducerContext, repo: &FileRepo, base: &str, head: &str, except: Option<&str>) -> Res<()> {
    let found = ctx.db.git_issues().repo_id().filter(repo.id.as_str()).any(|r| r.kind == "pull" && r.state == "open" && r.base == base && r.head == head && Some(r.id.as_str()) != except);
    if found { fail("alreadyExists") } else { Ok(()) }
}

/// Open counts for the Git app's tabs.
pub fn open_counts(ctx: &ReducerContext, repo_id: &str) -> Value {
    let (mut issues, mut pulls) = (0, 0);
    for row in ctx.db.git_issues().repo_id().filter(repo_id) {
        if row.state == "open" {
            if row.kind == "pull" {
                pulls += 1;
            } else {
                issues += 1;
            }
        }
    }
    json!({ "openIssues": issues, "openPulls": pulls })
}

/// Removes a repository's issues, pull requests and labels with it.
pub fn drop_tracker(ctx: &ReducerContext, repo_id: &str) {
    for row in ctx.db.git_comments().repo_id().filter(repo_id).collect::<Vec<_>>() {
        ctx.db.git_comments().id().delete(row.id);
    }
    for row in ctx.db.git_issues().repo_id().filter(repo_id).collect::<Vec<_>>() {
        ctx.db.git_issues().id().delete(row.id);
    }
    for row in label_rows(ctx, repo_id) {
        ctx.db.git_labels().id().delete(row.id);
    }
}

/// After a push changed refs (git.rs): open pull requests whose base now contains their head are
/// merged; one whose head branch was deleted is closed; one whose head moved records the push.
/// `tip(name, after)` is a branch's commit before or after the push.
pub fn pulls_after_push(ctx: &ReducerContext, files: &HistoryFiles, repo: &FileRepo, user_id: &str, tip: &dyn Fn(&str, bool) -> Option<String>, is_ancestor: &dyn Fn(&str, &str) -> bool) -> Res<()> {
    let at = now(ctx);
    for row in ctx.db.git_issues().repo_id().filter(repo.id.as_str()).collect::<Vec<_>>() {
        if row.kind != "pull" || row.state != "open" || agent_head(&row.head) {
            continue;
        }
        let head_before = tip(&row.head, false);
        let head_after = tip(&row.head, true);
        let base_before = tip(&row.base, false);
        let base_after = tip(&row.base, true);
        let head = head_after.clone().or(head_before.clone());
        // Merged by this push: the base contains the head now and did not before.
        if let (Some(head), Some(base_after)) = (&head, &base_after)
            && Some(base_after) != base_before.as_ref()
            && !base_before.as_ref().is_some_and(|b| is_ancestor(head, b))
            && is_ancestor(head, base_after)
        {
            merged(ctx, files, row, user_id, at, base_before.as_deref().unwrap_or(""), head, base_after);
            continue;
        }
        match (&head_before, &head_after) {
            (Some(before), None) => {
                let base_sha = base_after.clone().unwrap_or_default();
                let head_sha = before.clone();
                let from = row.head.clone();
                let closed = touch(ctx, GitIssue { state: "closed".into(), closed_at: at, closed_by: user_id.into(), base_sha, head_sha, ..row }, at);
                add_event(ctx, files, &closed, "branchDeleted", user_id, at, json!({ "from": from }));
            }
            (Some(before), Some(after)) if before != after => {
                add_event(ctx, files, &row, "pushed", user_id, at, json!({ "from": before, "to": after }));
                touch(ctx, row, at);
            }
            _ => {}
        }
    }
    Ok(())
}

/// An Agent branch was merged into the folder (history.rs) or discarded.
pub fn agent_branch_done(ctx: &ReducerContext, files: &HistoryFiles, repo_id: &str, name: &str, user_id: &str, merge: bool) -> Res<()> {
    let at = now(ctx);
    for row in ctx.db.git_issues().repo_id().filter(repo_id).collect::<Vec<_>>() {
        if row.kind != "pull" || row.state != "open" || row.head != name {
            continue;
        }
        if merge {
            merged(ctx, files, row, user_id, at, "", "", "");
        } else {
            let closed = touch(ctx, GitIssue { state: "closed".into(), closed_at: at, closed_by: user_id.into(), ..row }, at);
            add_event(ctx, files, &closed, "branchDeleted", user_id, at, json!({ "from": name }));
        }
    }
    Ok(())
}

/// `service_git_tracker`: browser sessions only.
pub fn tracker_operation(ctx: &ReducerContext, files: &HistoryFiles, user_id: &str, operation: &str, p: &Doc) -> Res<Value> {
    let (repo, role) = repo_for(ctx, files, p)?;
    let at = now(ctx);
    let need_write = || if writer(role) { Ok(()) } else { fail("forbidden") };
    match operation {
        "list" => {
            keys(p, &["folderId", "kind", "state", "label"])?;
            let kind = match p.get("kind").and_then(Value::as_str) {
                Some(k @ ("pull" | "issue")) => k,
                _ => return fail("invalidInput"),
            };
            let closed = match p.get("state").and_then(Value::as_str) {
                Some("closed") => true,
                Some("open") => false,
                _ => return fail("invalidInput"),
            };
            let label = match p.get("label") {
                None => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return fail("invalidInput"),
            };
            let (mut open, mut shut) = (0, 0);
            let mut rows: Vec<GitIssue> = Vec::new();
            for row in ctx.db.git_issues().repo_id().filter(repo.id.as_str()) {
                // An empty label filter matches everything, as in TypeScript.
                if row.kind != kind || label.as_ref().is_some_and(|l| !l.is_empty() && !label_list(&row.labels).contains(l)) {
                    continue;
                }
                if row.state == "open" {
                    open += 1;
                } else {
                    shut += 1;
                }
                if (row.state != "open") == closed {
                    rows.push(row);
                }
            }
            rows.sort_by(|a, b| b.number.cmp(&a.number));
            Ok(json!({ "items": rows.iter().take(PAGE).map(|r| summary(ctx, r)).collect::<Vec<_>>(), "open": open, "closed": shut, "labels": sorted_labels(ctx, &repo.id), "truncated": rows.len() > PAGE }))
        }
        "issue" => {
            keys(p, &["folderId", "number"])?;
            let row = issue_of(ctx, &repo, p.get("number"))?;
            Ok(detail(ctx, &repo, role, user_id, &row))
        }
        "labels" => {
            keys(p, &["folderId"])?;
            Ok(sorted_labels(ctx, &repo.id))
        }
        "create" => {
            keys(p, &["folderId", "kind", "title", "body", "labels", "base", "head"])?;
            let kind = match p.get("kind").and_then(Value::as_str) {
                Some(k @ ("pull" | "issue")) => k,
                _ => return fail("invalidInput"),
            };
            let rows: Vec<GitIssue> = ctx.db.git_issues().repo_id().filter(repo.id.as_str()).collect();
            if rows.len() >= ITEMS {
                return fail("operationTooLarge");
            }
            let labels = if p.contains_key("labels") {
                need_write()?;
                chosen_labels(ctx, &repo, p.get("labels"))?
            } else {
                Vec::new()
            };
            let (mut base, mut head) = (String::new(), String::new());
            if kind == "pull" {
                need_write()?;
                base = branch_name(p.get("base"))?;
                head = branch_name(p.get("head"))?;
                if base == head || agent_head(&base) || agent_head(&head) && base != "main" {
                    return fail("invalidInput");
                }
                if !branch_exists(ctx, &repo, &base) || !branch_exists(ctx, &repo, &head) {
                    return fail("notFound");
                }
                duplicate(ctx, &repo, &base, &head, None)?;
            } else if p.contains_key("base") || p.contains_key("head") {
                return fail("invalidInput");
            }
            let number = rows.iter().map(|r| r.number).max().unwrap_or(0) + 1;
            let row = GitIssue {
                id: issue_id(&repo.id, number),
                repo_id: repo.id.clone(),
                number,
                kind: kind.into(),
                title: title(p.get("title"))?,
                body: body(p.get("body"))?,
                state: "open".into(),
                author_id: user_id.into(),
                created_at: at,
                updated_at: at,
                closed_at: 0.0,
                closed_by: String::new(),
                labels: labels.join(" "),
                base,
                head,
                base_sha: String::new(),
                head_sha: String::new(),
                merged_sha: String::new(),
                revision: 1,
            };
            let row = ctx.db.git_issues().insert(row);
            mention(ctx, files, &row, &row.body, user_id, at);
            Ok(json!({ "number": number }))
        }
        "update" => {
            keys(p, &["folderId", "number", "revision", "title", "body", "state", "labels"])?;
            let mut row = issue_of(ctx, &repo, p.get("number"))?;
            if p.get("revision").and_then(Value::as_f64) != Some(row.revision as f64) {
                return fail("conflict");
            }
            let own = row.author_id == user_id;
            if (p.contains_key("title") || p.contains_key("body")) && !own && !writer(role) {
                return fail("forbidden");
            }
            if p.contains_key("title") {
                let next = title(p.get("title"))?;
                if next != row.title {
                    add_event(ctx, files, &row, "renamed", user_id, at, json!({ "from": row.title, "to": next }));
                    row.title = next;
                }
            }
            if p.contains_key("body") {
                let next = body(p.get("body"))?;
                if next != row.body {
                    row.body = next.clone();
                    mention(ctx, files, &row, &next, user_id, at);
                }
            }
            if p.contains_key("labels") {
                need_write()?;
                let next = chosen_labels(ctx, &repo, p.get("labels"))?;
                let before = label_list(&row.labels);
                let names: HashMap<String, String> = label_rows(ctx, &repo.id).into_iter().map(|l| (l.id, l.name)).collect();
                for id in &next {
                    if !before.contains(id) {
                        add_event(ctx, files, &row, "labeled", user_id, at, json!({ "label": names.get(id) }));
                    }
                }
                for id in &before {
                    if !next.contains(id) && names.contains_key(id) {
                        add_event(ctx, files, &row, "unlabeled", user_id, at, json!({ "label": names.get(id) }));
                    }
                }
                row.labels = next.join(" ");
            }
            if let Some(state) = p.get("state")
                && state.as_str() != Some(row.state.as_str())
            {
                let state = state.as_str().filter(|s| *s == "open" || *s == "closed");
                let Some(state) = state.filter(|_| row.state != "merged") else { return fail("invalidInput") };
                if if row.kind == "pull" { !writer(role) } else { !own && !writer(role) } {
                    return fail("forbidden");
                }
                if state == "closed" {
                    // A closed pull request keeps the tips it had, so its commits stay readable while they exist.
                    if row.kind == "pull" && !agent_head(&row.head) {
                        row.base_sha = tip_of(ctx, &repo, &row.base).unwrap_or_default();
                        row.head_sha = tip_of(ctx, &repo, &row.head).unwrap_or_default();
                    }
                    row.state = "closed".into();
                    row.closed_at = at;
                    row.closed_by = user_id.into();
                    add_event(ctx, files, &row, "closed", user_id, at, json!({}));
                } else {
                    if row.kind == "pull" {
                        if !branch_exists(ctx, &repo, &row.base) || !branch_exists(ctx, &repo, &row.head) {
                            return fail("conflict");
                        }
                        duplicate(ctx, &repo, &row.base, &row.head, Some(&row.id))?;
                    }
                    row.state = "open".into();
                    row.closed_at = 0.0;
                    row.closed_by = String::new();
                    row.base_sha = String::new();
                    row.head_sha = String::new();
                    add_event(ctx, files, &row, "reopened", user_id, at, json!({}));
                }
            }
            Ok(json!({ "revision": touch(ctx, row, at).revision }))
        }
        "comment" => {
            keys(p, &["folderId", "number", "body"])?;
            let row = issue_of(ctx, &repo, p.get("number"))?;
            let text = body(p.get("body"))?;
            if text.is_empty() {
                return fail("invalidInput");
            }
            if ctx.db.git_comments().issue_id().filter(row.id.as_str()).count() >= COMMENTS {
                return fail("operationTooLarge");
            }
            let id = files.uuid(ctx);
            ctx.db.git_comments().insert(GitComment { id: id.clone(), issue_id: row.id.clone(), repo_id: repo.id.clone(), sequence: next_sequence(ctx, &row.id), kind: "comment".into(), author_id: user_id.into(), body: text.clone(), data: String::new(), created_at: at, edited_at: 0.0 });
            let row = touch(ctx, row, at);
            mention(ctx, files, &row, &text, user_id, at);
            Ok(json!({ "id": id }))
        }
        "editComment" | "deleteComment" => {
            if operation == "editComment" {
                keys(p, &["folderId", "id", "body"])?;
            } else {
                keys(p, &["folderId", "id"])?;
            }
            let item = p.get("id").and_then(Value::as_str).and_then(|id| ctx.db.git_comments().id().find(id.to_owned()));
            let Some(item) = item.filter(|i| i.repo_id == repo.id && i.kind == "comment") else { return fail("notFound") };
            let row = ctx.db.git_issues().id().find(item.issue_id.clone()).expect("issue");
            if operation == "deleteComment" {
                if item.author_id != user_id && !writer(role) {
                    return fail("forbidden");
                }
                ctx.db.git_comments().id().delete(item.id);
                touch(ctx, row, at);
                return Ok(json!({ "deleted": true }));
            }
            if item.author_id != user_id {
                return fail("forbidden");
            }
            let text = body(p.get("body"))?;
            if text.is_empty() {
                return fail("invalidInput");
            }
            let id = item.id.clone();
            ctx.db.git_comments().id().update(GitComment { body: text.clone(), edited_at: at, ..item });
            let row = touch(ctx, row, at);
            mention(ctx, files, &row, &text, user_id, at);
            Ok(json!({ "id": id }))
        }
        "saveLabel" => {
            keys(p, &["folderId", "id", "name", "color", "description"])?;
            need_write()?;
            let name = p.get("name").and_then(Value::as_str).map(squash).unwrap_or_default();
            let description = match p.get("description") {
                None => String::new(),
                Some(Value::String(s)) => squash(s),
                Some(_) => return fail("invalidInput"),
            };
            let color = p.get("color").and_then(Value::as_str).filter(|c| COLORS.contains(c));
            let Some(color) = color.filter(|_| !name.is_empty() && utf16(&name) <= LABEL_NAME && utf16(&description) <= LABEL_DESCRIPTION) else { return fail("invalidInput") };
            let rows = label_rows(ctx, &repo.id);
            let existing = match p.get("id") {
                None => None,
                Some(id) => match rows.iter().find(|r| Some(r.id.as_str()) == id.as_str()) {
                    Some(r) => Some(r.clone()),
                    None => return fail("notFound"),
                },
            };
            let existing_id = existing.as_ref().map(|e| e.id.clone());
            if rows.iter().any(|r| Some(&r.id) != existing_id.as_ref() && r.name.to_lowercase() == name.to_lowercase()) {
                return fail("alreadyExists");
            }
            if existing.is_none() && rows.len() >= LABELS {
                return fail("operationTooLarge");
            }
            let row = GitLabel { id: existing_id.clone().unwrap_or_else(|| files.uuid(ctx)), repo_id: repo.id.clone(), name, color: color.into(), description, created_at: existing.as_ref().map_or(at, |e| e.created_at) };
            if existing.is_some() {
                ctx.db.git_labels().id().update(row.clone());
            } else {
                ctx.db.git_labels().insert(row.clone());
            }
            Ok(label_view(&row))
        }
        "deleteLabel" => {
            keys(p, &["folderId", "id"])?;
            need_write()?;
            let Some(label) = label_rows(ctx, &repo.id).into_iter().find(|r| Some(r.id.as_str()) == p.get("id").and_then(Value::as_str)) else { return fail("notFound") };
            for row in ctx.db.git_issues().repo_id().filter(repo.id.as_str()).collect::<Vec<_>>() {
                let list = label_list(&row.labels);
                if list.contains(&label.id) {
                    let labels = list.into_iter().filter(|id| *id != label.id).collect::<Vec<_>>().join(" ");
                    let revision = row.revision + 1;
                    ctx.db.git_issues().id().update(GitIssue { labels, revision, ..row });
                }
            }
            ctx.db.git_labels().id().delete(label.id);
            Ok(json!({ "deleted": true }))
        }
        _ => fail("invalidInput"),
    }
}
