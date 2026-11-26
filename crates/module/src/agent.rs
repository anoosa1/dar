//! Agent (`agent.ts`, `agent-runs.ts`): projects, threads and messages, shared threads, and runs
//! a service-only executor claims with a lease; a stale process cannot commit results.

use serde_json::{Map, Value, json};
use spacetimedb::{ProcedureContext, ReducerContext, Table};

use crate::app_settings::app_enabled;
use crate::files::{Unlock, access};
use crate::tables::*;
use crate::{Res, fail, now, row};

type Doc = Map<String, Value>;

const PROJECTS: usize = 32;
const THREADS: usize = 128;
const MESSAGES: usize = 1000;
const MESSAGE_BYTES: usize = 16384;
const PAGE: usize = 50;
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
pub const READ_OPERATIONS: [&str; 9] = [
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

pub fn active_run(status: &str) -> bool {
    ["queued", "running", "cancelling"].contains(&status)
}

fn valid_effort(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase())
}

fn text(value: Option<&Value>, max: usize) -> Res<String> {
    let Some(s) = value.and_then(Value::as_str) else {
        return fail("invalidInput");
    };
    if s.trim().is_empty() || s.contains('\0') || s.len() > max {
        return fail("invalidInput");
    }
    Ok(s.to_owned())
}

fn id(value: Option<&Value>) -> Res<String> {
    match value.and_then(Value::as_str) {
        Some(s) if crate::opaque_id(s).is_ok() => Ok(s.to_owned()),
        _ => fail("invalidInput"),
    }
}

fn keys(p: &Doc, allowed: &[&str]) -> Res<()> {
    if p.keys().any(|k| !allowed.contains(&k.as_str())) {
        fail("invalidInput")
    } else {
        Ok(())
    }
}

/// `validRepositoryUrl`: an https:// or git@host: repository address.
pub fn valid_repository_url(value: &str) -> bool {
    let host_ok = |h: &str| {
        h.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && h.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    };
    if let Some(rest) = value.strip_prefix("https://") {
        let Some((authority, path)) = rest.split_once('/') else {
            return false;
        };
        let (host, port) = match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };
        return host_ok(host)
            && port.is_none_or(|p| {
                (1..=5).contains(&p.len()) && p.chars().all(|c| c.is_ascii_digit())
            })
            && !path.is_empty()
            && path
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._~/-".contains(c));
    }
    if let Some(rest) = value.strip_prefix("git@") {
        let Some((host, path)) = rest.split_once(':') else {
            return false;
        };
        return host_ok(host)
            && !path.is_empty()
            && path
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c));
    }
    false
}

fn run_summary(r: &AgentRun) -> Value {
    json!({ "id": r.id, "threadId": r.thread_id, "messageId": r.message_id, "model": r.model, "status": r.status, "createdAt": r.created_at, "modifiedAt": r.modified_at })
}

fn expose_run(r: &AgentRun) -> Value {
    let mut v = run_summary(r);
    v["document"] = serde_json::from_str(&r.document).unwrap_or(Value::Null);
    v
}

fn empty_run_document() -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in [
        ("output", json!("")),
        ("terminal", json!("")),
        ("activity", json!([])),
        ("changes", json!([])),
        ("error", json!("")),
        ("truncated", json!(false)),
    ] {
        m.insert(k.into(), v);
    }
    m
}

fn is_hex_uuid_like(s: &str) -> bool {
    s.len() == 36
        && s.bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

pub fn valid_attachments(value: &Value) -> bool {
    let Some(list) = value.as_array() else {
        return false;
    };
    list.len() <= 4
        && list.iter().all(|item| {
            let Some(o) = item.as_object() else {
                return false;
            };
            let name = o.get("name").and_then(Value::as_str).unwrap_or_default();
            o.keys()
                .all(|k| ["id", "name", "mediaType", "size"].contains(&k.as_str()))
                && o.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(is_hex_uuid_like)
                && !name.is_empty()
                && utf16_len(name) <= 200
                && !name
                    .chars()
                    .any(|c| (c as u32) < 0x20 || c == '/' || c == '\\')
                && o.get("mediaType")
                    .and_then(Value::as_str)
                    .is_some_and(|t| IMAGE_TYPES.contains(&t))
                && o.get("size")
                    .and_then(Value::as_f64)
                    .is_some_and(|s| s.fract() == 0.0 && s > 0.0 && s <= 8.0 * 1024.0 * 1024.0)
        })
}

fn safe_int(v: Option<&Value>) -> bool {
    v.and_then(Value::as_f64)
        .is_some_and(|f| f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_991.0)
}

fn short(v: Option<&Value>, max: usize) -> bool {
    v.and_then(Value::as_str)
        .is_some_and(|s| utf16_len(s) <= max)
}

/// `validateRunDocument`.
pub fn validate_run_document(value: &Value) -> Res<Value> {
    let bad = || -> Res<Value> { fail("invalidInput") };
    let Some(data) = value.as_object() else {
        return bad();
    };
    const KEYS: [&str; 18] = [
        "output",
        "terminal",
        "activity",
        "changes",
        "error",
        "truncated",
        "steps",
        "usage",
        "effort",
        "mode",
        "attachments",
        "steered",
        "pending",
        "codexThread",
        "claudeSession",
        "contextRevision",
        "contextWindow",
        "contextUsed",
    ];
    let activity = data.get("activity").and_then(Value::as_array);
    let changes = data.get("changes").and_then(Value::as_array);
    if data.keys().any(|k| !KEYS.contains(&k.as_str()))
        || !short(data.get("output"), 65536)
        || !short(data.get("terminal"), 65536)
        || activity.is_none_or(|a| a.len() > 100 || a.iter().any(|s| !short(Some(s), 500)))
        || changes.is_none_or(|c| c.len() > 200)
        || !short(data.get("error"), 2000)
        || !data.get("truncated").is_some_and(Value::is_boolean)
    {
        return bad();
    }
    if let Some(e) = data.get("effort")
        && !e.as_str().is_some_and(valid_effort)
    {
        return bad();
    }
    if let Some(usage) = data.get("usage") {
        let Some(u) = usage.as_object() else {
            return bad();
        };
        if u.keys()
            .any(|k| !["input", "cached", "output"].contains(&k.as_str()))
            || ["input", "cached", "output"]
                .iter()
                .any(|k| !safe_int(u.get(*k)) || u[*k].as_f64().unwrap_or(-1.0) < 0.0)
        {
            return bad();
        }
    }
    if let Some(mode) = data.get("mode")
        && !["plan", "default", "review", "compact"].contains(&crate::text_of(Some(mode)).as_str())
    {
        return bad();
    }
    if let Some(t) = data.get("codexThread")
        && !t.as_str().is_some_and(|s| {
            (1..=100).contains(&s.len())
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return bad();
    }
    if let Some(t) = data.get("claudeSession")
        && !t.as_str().is_some_and(|s| crate::opaque_id(s).is_ok())
    {
        return bad();
    }
    for k in ["contextRevision", "contextWindow", "contextUsed"] {
        if let Some(v) = data.get(k)
            && (!safe_int(Some(v)) || v.as_f64().unwrap_or(-1.0) < 0.0)
        {
            return bad();
        }
    }
    if let Some(s) = data.get("steered")
        && !s.as_array().is_some_and(|a| {
            a.len() <= 50 && a.iter().all(|v| v.as_str().is_some_and(is_hex_uuid_like))
        })
    {
        return bad();
    }
    if let Some(a) = data.get("attachments")
        && !valid_attachments(a)
    {
        return bad();
    }
    if let Some(pending) = data.get("pending").filter(|p| !p.is_null()) {
        let ok = pending.as_object().is_some_and(|p| {
            p.keys().all(|k| k == "id" || k == "questions")
                && short(p.get("id"), 200)
                && p.get("questions")
                    .and_then(Value::as_array)
                    .is_some_and(|qs| {
                        qs.len() <= 10
                            && qs.iter().all(|q| {
                                q.as_object().is_some_and(|q| {
                                    q.keys().all(|k| {
                                        ["id", "header", "question", "options", "other", "secret"]
                                            .contains(&k.as_str())
                                    }) && short(q.get("id"), 200)
                                        && short(q.get("header"), 200)
                                        && short(q.get("question"), 2000)
                                        && q.get("other").is_some_and(Value::is_boolean)
                                        && q.get("secret").is_some_and(Value::is_boolean)
                                        && q.get("options").and_then(Value::as_array).is_some_and(
                                            |os| {
                                                os.len() <= 20
                                                    && os.iter().all(|o| {
                                                        o.as_object().is_some_and(|o| {
                                                            o.keys().all(|k| {
                                                                k == "label" || k == "description"
                                                            }) && short(o.get("label"), 200)
                                                                && short(o.get("description"), 1000)
                                                        })
                                                    })
                                            },
                                        )
                                })
                            })
                    })
        });
        if !ok {
            return bad();
        }
    }
    if let Some(steps) = data.get("steps") {
        let Some(steps) = steps.as_array().filter(|s| s.len() <= 100) else {
            return bad();
        };
        for step in steps {
            let Some(s) = step.as_object() else {
                return bad();
            };
            if s.keys().any(|k| {
                ![
                    "id", "kind", "label", "status", "exitCode", "output", "items",
                ]
                .contains(&k.as_str())
            }) || !short(s.get("id"), 200)
                || ![
                    "command",
                    "edit",
                    "reasoning",
                    "plan",
                    "tool",
                    "search",
                    "question",
                ]
                .contains(&crate::text_of(s.get("kind")).as_str())
                || !short(s.get("label"), 2000)
                || !["running", "completed", "failed"]
                    .contains(&crate::text_of(s.get("status")).as_str())
            {
                return bad();
            }
            if s.contains_key("exitCode") && !safe_int(s.get("exitCode"))
                || s.contains_key("output") && !short(s.get("output"), 2000)
            {
                return bad();
            }
            if let Some(items) = s.get("items")
                && !items.as_array().is_some_and(|a| {
                    a.len() <= 50
                        && a.iter().all(|i| {
                            i.as_object().is_some_and(|i| {
                                i.keys().all(|k| k == "text" || k == "done")
                                    && short(i.get("text"), 500)
                                    && i.get("done").is_some_and(Value::is_boolean)
                            })
                        })
                })
            {
                return bad();
            }
        }
    }
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    for change in changes.expect("changes") {
        let Some(c) = change.as_object() else {
            return bad();
        };
        let path = c.get("path").and_then(Value::as_str);
        if c.keys().any(|k| {
            ![
                "path",
                "kind",
                "diff",
                "content",
                "fileId",
                "expectedRevision",
                "mediaType",
            ]
            .contains(&k.as_str())
        }) || path.is_none_or(|p| {
            utf16_len(p) > 2048
                || p.split('/').any(|part| {
                    part.is_empty()
                        || part == "."
                        || part == ".."
                        || part.chars().any(|ch| (ch as u32) < 0x20 || ch == '\\')
                })
        }) || !["added", "modified", "deleted"].contains(&crate::text_of(c.get("kind")).as_str())
            || !short(c.get("diff"), 32768)
        {
            return bad();
        }
        if let Some(m) = c.get("mediaType")
            && !m.as_str().is_some_and(|m| {
                utf16_len(m) <= 150 && m.split_once('/').is_some_and(|(a, b)| token(a) && token(b))
            })
        {
            return bad();
        }
        if c.contains_key("content") {
            let ok = short(c.get("content"), 65536)
                && c.get("fileId")
                    .and_then(Value::as_str)
                    .is_some_and(is_hex_uuid_like)
                && c.get("expectedRevision")
                    .and_then(Value::as_f64)
                    .is_some_and(|r| r.fract() == 0.0 && (1.0..=4_294_967_295.0).contains(&r))
                && c.get("kind").and_then(Value::as_str) == Some("modified");
            if !ok {
                return bad();
            }
        }
    }
    Ok(value.clone())
}

fn new_uuid(ctx: &ReducerContext, taken: &dyn Fn(&str) -> bool) -> String {
    loop {
        let id = ctx.new_uuid_v4().expect("randomness").to_string();
        if !taken(&id) {
            return id;
        }
    }
}

fn message_json(m: &AgentMessage) -> Value {
    row(m)
}

/// All ownership, parent, quota and revision checks share the mutation transaction.
pub fn agent_operation(
    ctx: &ReducerContext,
    user_id: &str,
    operation: &str,
    p: &Doc,
    can_read: &dyn Fn(&FileEntry) -> bool,
    session_id: &str,
) -> Res<Value> {
    if !app_enabled(ctx, "agent") {
        return fail("notFound");
    }
    let at = now(ctx);
    let project = |value: Option<&Value>| -> Res<AgentProject> {
        ctx.db
            .agent_projects()
            .id()
            .find(id(value)?)
            .filter(|r| r.owner_id == user_id)
            .ok_or_else(|| "notFound".to_owned())
    };
    let thread = |value: Option<&Value>| -> Res<AgentThread> {
        ctx.db
            .agent_threads()
            .id()
            .find(id(value)?)
            .filter(|r| r.owner_id == user_id)
            .ok_or_else(|| "notFound".to_owned())
    };
    match operation {
        "run" | "cancelRun" => {
            keys(p, &["id"])?;
            let Some(r) = ctx
                .db
                .agent_runs()
                .id()
                .find(id(p.get("id"))?)
                .filter(|r| r.owner_id == user_id)
            else {
                return fail("notFound");
            };
            if operation == "cancelRun" && active_run(&r.status) {
                let next = AgentRun {
                    status: if r.status == "queued" {
                        "cancelled".into()
                    } else {
                        "cancelling".into()
                    },
                    modified_at: at,
                    ..r
                };
                ctx.db.agent_runs().id().update(next.clone());
                return Ok(expose_run(&next));
            }
            return Ok(expose_run(&r));
        }
        "source" => {
            keys(p, &["projectId"])?;
            let parent = project(p.get("projectId"))?;
            if parent.source_type != "files" {
                return Ok(json!({ "project": row(&parent), "files": [] }));
            }
            let Some(root) = ctx
                .db
                .file_entries()
                .id()
                .find(parent.source_ref.clone())
                .filter(|r| r.kind == "folder" && can_read(r))
            else {
                return fail("notFound");
            };
            let mut folders: Vec<(FileEntry, String, u32)> = vec![(root, String::new(), 0)];
            let mut files = Vec::new();
            let (mut bytes, mut scanned) = (0.0, 0);
            let mut i = 0;
            while i < folders.len() {
                let (folder, path, depth) = folders[i].clone();
                i += 1;
                let window = crate::folder_window(&format!("{}/{}/", folder.owner_id, folder.id));
                for entry in ctx
                    .db
                    .file_entries()
                    .list_key()
                    .filter(crate::bounds(&window))
                {
                    scanned += 1;
                    if scanned > 500 || depth > 32 {
                        return fail("operationTooLarge");
                    }
                    if !can_read(&entry) || [".git", ".codex"].contains(&entry.name.as_str()) {
                        continue;
                    }
                    let child = format!("{path}{}", entry.name);
                    if utf16_len(&child) > 2048 {
                        return fail("operationTooLarge");
                    }
                    if entry.kind == "folder" {
                        folders.push((entry, format!("{child}/"), depth + 1));
                    } else {
                        bytes += entry.size;
                        if files.len() >= 200
                            || entry.size > 1024.0 * 1024.0
                            || bytes > 8.0 * 1024.0 * 1024.0
                        {
                            return fail("operationTooLarge");
                        }
                        files.push(json!({ "id": entry.id, "path": child, "revision": entry.revision, "size": entry.size, "mediaType": entry.media_type }));
                    }
                }
            }
            return Ok(json!({ "project": row(&parent), "files": files }));
        }
        "folders" => {
            keys(p, &["parentId", "cursor"])?;
            let parent_id = if p.get("parentId").is_some_and(|v| {
                !v.is_null() && v.as_str() != Some("") && v != &json!(false) && v != &json!(0)
            }) {
                id(p.get("parentId"))?
            } else {
                String::new()
            };
            let parent = if parent_id.is_empty() {
                None
            } else {
                ctx.db.file_entries().id().find(parent_id.clone())
            };
            if !parent_id.is_empty()
                && parent
                    .as_ref()
                    .is_none_or(|f| f.kind != "folder" || !can_read(f))
            {
                return fail("notFound");
            }
            let prefix = format!(
                "{}/{parent_id}/",
                parent
                    .as_ref()
                    .map_or(user_id.to_owned(), |f| f.owner_id.clone())
            );
            let cursor = p
                .get("cursor")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if !cursor.is_empty() && !cursor.starts_with(&prefix) {
                return fail("invalidInput");
            }
            let lower = if cursor.is_empty() {
                std::ops::Bound::Included(prefix.clone())
            } else {
                std::ops::Bound::Excluded(cursor.clone())
            };
            let range = (
                lower,
                std::ops::Bound::Excluded(format!("{}0", &prefix[..prefix.len() - 1])),
            );
            let mut items = Vec::new();
            let mut next = Value::Null;
            for (scanned, r) in ctx
                .db
                .file_entries()
                .list_key()
                .filter(crate::bounds(&range))
                .enumerate()
            {
                if r.kind == "folder" && can_read(&r) {
                    items.push(json!({ "id": r.id, "name": r.name }));
                }
                if scanned + 1 >= 200 {
                    next = json!(r.list_key);
                    break;
                }
            }
            return Ok(
                json!({ "folder": parent.map(|f| json!({ "id": f.id, "name": f.name, "parentId": f.parent_id })), "items": items, "nextCursor": next }),
            );
        }
        "sharedThreads" => {
            keys(p, &[])?;
            let mut items: Vec<(f64, Value)> = Vec::new();
            for share in ctx.db.agent_shares().grantee_id().filter(user_id) {
                let Some(t) = ctx.db.agent_threads().id().find(share.thread_id.clone()) else {
                    continue;
                };
                let Some(owner) = ctx
                    .db
                    .accounts()
                    .id()
                    .find(t.owner_id.clone())
                    .filter(|o| o.enabled)
                else {
                    continue;
                };
                if t.owner_id != share.owner_id {
                    continue;
                }
                let owner_name = if owner.display_name.is_empty() {
                    owner.username.clone()
                } else {
                    owner.display_name.clone()
                };
                items.push((t.modified_at, json!({ "shareId": share.id, "thread": { "id": t.id, "title": t.title, "revision": t.revision, "modifiedAt": t.modified_at },
                    "owner": owner_name, "project": ctx.db.agent_projects().id().find(t.project_id.clone()).map(|x| x.name).unwrap_or_else(|| "Project".into()) })));
            }
            items.sort_by(|a, b| b.0.total_cmp(&a.0));
            return Ok(json!({ "items": items.into_iter().map(|x| x.1).collect::<Vec<_>>() }));
        }
        "sharedHistory" => {
            keys(p, &["threadId", "before"])?;
            let thread_id = id(p.get("threadId"))?;
            let share = ctx
                .db
                .agent_shares()
                .thread_id()
                .filter(thread_id.as_str())
                .find(|s| s.grantee_id == user_id);
            let t = ctx.db.agent_threads().id().find(thread_id.clone());
            let owner = t
                .as_ref()
                .and_then(|t| ctx.db.accounts().id().find(t.owner_id.clone()));
            let (Some(share), Some(t), Some(owner)) = (share, t, owner) else {
                return fail("notFound");
            };
            if t.owner_id != share.owner_id || !owner.enabled {
                return fail("notFound");
            }
            let before = before_of(p)?;
            let mut rows: Vec<AgentMessage> = ctx
                .db
                .agent_messages()
                .thread_id()
                .filter(t.id.as_str())
                .filter(|m| (m.sequence as f64) < before)
                .collect();
            rows.sort_by(|a, b| b.sequence.cmp(&a.sequence));
            let mut items: Vec<AgentMessage> = rows.iter().take(PAGE).cloned().collect();
            items.reverse();
            let next = if rows.len() > PAGE {
                json!(items[0].sequence)
            } else {
                Value::Null
            };
            let owner_name = if owner.display_name.is_empty() {
                owner.username.clone()
            } else {
                owner.display_name.clone()
            };
            return Ok(
                json!({ "thread": { "id": t.id, "title": t.title, "revision": t.revision, "modifiedAt": t.modified_at }, "owner": owner_name,
                "project": ctx.db.agent_projects().id().find(t.project_id.clone()).map(|x| x.name).unwrap_or_else(|| "Project".into()),
                "items": items.iter().map(|m| { let mut v = message_json(m); v.as_object_mut().expect("row").remove("ownerId"); v }).collect::<Vec<_>>(), "nextBefore": next }),
            );
        }
        "projects" => {
            keys(p, &[])?;
            let mut list: Vec<AgentProject> =
                ctx.db.agent_projects().owner_id().filter(user_id).collect();
            list.sort_by(|a, b| {
                crate::locale_compare(&a.name, &b.name)
                    .then_with(|| crate::locale_compare(&a.id, &b.id))
            });
            return Ok(json!({ "items": list.iter().map(|x| row(x)).collect::<Vec<_>>() }));
        }
        "createProject" => {
            keys(p, &["id", "name", "sourceType", "sourceRef"])?;
            let source_ref = text(p.get("sourceRef"), 2048)?;
            let source_type = crate::text_of(p.get("sourceType"));
            match source_type.as_str() {
                "git" => {
                    if !valid_repository_url(&source_ref) {
                        return fail("invalidInput");
                    }
                }
                "files" => {
                    let folder = ctx
                        .db
                        .file_entries()
                        .id()
                        .find(id(Some(&json!(source_ref)))?);
                    if folder.is_none_or(|f| f.kind != "folder" || !can_read(&f)) {
                        return fail("notFound");
                    }
                }
                "scratch" => {
                    if source_ref != "scratch" {
                        return fail("invalidInput");
                    }
                }
                _ => return fail("invalidInput"),
            }
            let created = AgentProject {
                id: id(p.get("id"))?,
                owner_id: user_id.into(),
                name: text(p.get("name"), 160)?.trim().to_owned(),
                source_type,
                source_ref,
                created_at: at,
            };
            if let Some(old) = ctx.db.agent_projects().id().find(created.id.clone()) {
                if old.owner_id != user_id
                    || old.name != created.name
                    || old.source_type != created.source_type
                    || old.source_ref != created.source_ref
                {
                    return fail("conflict");
                }
                return Ok(row(&old));
            }
            if ctx.db.agent_projects().owner_id().filter(user_id).count() >= PROJECTS {
                return fail("quotaExceeded");
            }
            ctx.db.agent_projects().insert(created.clone());
            return Ok(row(&created));
        }
        "threads" => {
            keys(p, &["projectId"])?;
            let has_project = p.get("projectId").is_some_and(|v| {
                !v.is_null() && v.as_str() != Some("") && v != &json!(false) && v != &json!(0)
            });
            let mut rows: Vec<AgentThread> = if has_project {
                ctx.db
                    .agent_threads()
                    .project_id()
                    .filter(project(p.get("projectId"))?.id.as_str())
                    .collect()
            } else {
                ctx.db.agent_threads().owner_id().filter(user_id).collect()
            };
            rows.sort_by(|a, b| {
                b.modified_at
                    .total_cmp(&a.modified_at)
                    .then_with(|| crate::locale_compare(&a.id, &b.id))
            });
            let items: Vec<Value> = rows
                .iter()
                .map(|t| {
                    let mut runs: Vec<AgentRun> = ctx
                        .db
                        .agent_runs()
                        .thread_id()
                        .filter(t.id.as_str())
                        .collect();
                    runs.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
                    let mut v = row(t);
                    if let Some(r) = runs.first() {
                        v["runStatus"] = json!(r.status);
                    }
                    v
                })
                .collect();
            return Ok(json!({ "items": items }));
        }
        "createThread" => {
            keys(p, &["id", "projectId", "title"])?;
            let parent = project(p.get("projectId"))?;
            let title = text(Some(p.get("title").unwrap_or(&json!("New thread"))), 200)?
                .trim()
                .to_owned();
            let created = AgentThread {
                id: id(p.get("id"))?,
                owner_id: user_id.into(),
                project_id: parent.id.clone(),
                title,
                revision: 1,
                created_at: at,
                modified_at: at,
            };
            if let Some(old) = ctx.db.agent_threads().id().find(created.id.clone()) {
                if old.owner_id != user_id
                    || old.project_id != parent.id
                    || p.contains_key("title") && old.title != created.title
                {
                    return fail("conflict");
                }
                return Ok(row(&old));
            }
            if ctx.db.agent_threads().owner_id().filter(user_id).count() >= THREADS {
                return fail("quotaExceeded");
            }
            ctx.db.agent_threads().insert(created.clone());
            return Ok(row(&created));
        }
        _ => {}
    }
    let current = thread(p.get("threadId"))?;
    let owner_messages = || ctx.db.agent_messages().owner_id().filter(user_id).count();
    match operation {
        "startRun" => {
            keys(
                p,
                &[
                    "threadId",
                    "id",
                    "messageId",
                    "content",
                    "revision",
                    "model",
                    "effort",
                    "mode",
                    "attachments",
                ],
            )?;
            let mode = p.get("mode").map(|m| crate::text_of(Some(m)));
            if mode
                .as_ref()
                .is_some_and(|m| !["plan", "default", "review", "compact"].contains(&m.as_str()))
            {
                return fail("invalidInput");
            }
            if p.get("attachments").is_some_and(|a| !valid_attachments(a)) {
                return fail("invalidInput");
            }
            let run_id = id(p.get("id"))?;
            let message_id = id(p.get("messageId"))?;
            let content = text(p.get("content"), MESSAGE_BYTES)?;
            let model = text(p.get("model"), 120)?;
            let effort = match p.get("effort") {
                None => None,
                Some(Value::String(e)) if valid_effort(e) => Some(e.clone()),
                _ => return fail("invalidInput"),
            };
            if let Some(old) = ctx.db.agent_runs().id().find(run_id.clone()) {
                let message = ctx.db.agent_messages().id().find(old.message_id.clone());
                let document: Value = serde_json::from_str(&old.document).unwrap_or(json!({}));
                let same_effort = document
                    .get("effort")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    == effort;
                let old_mode = document
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("default")
                    .to_owned();
                if old.owner_id != user_id
                    || old.thread_id != current.id
                    || old.message_id != message_id
                    || old.model != model
                    || message.is_none_or(|m| m.content != content)
                    || !same_effort
                    || old_mode != mode.clone().unwrap_or_else(|| "default".into())
                {
                    return fail("conflict");
                }
                return Ok(expose_run(&old));
            }
            let runs: Vec<AgentRun> = ctx.db.agent_runs().owner_id().filter(user_id).collect();
            if runs.len() >= 100 {
                return fail("quotaExceeded");
            }
            // One run per account bounds provider and process usage as well as per-thread concurrency.
            if runs.iter().any(|r| active_run(&r.status)) {
                return fail("conflict");
            }
            if owner_messages() >= MESSAGES - 1 {
                return fail("quotaExceeded");
            }
            let mut append = Map::new();
            append.insert("threadId".into(), json!(current.id));
            append.insert("id".into(), json!(message_id));
            append.insert("content".into(), json!(content));
            if let Some(r) = p.get("revision") {
                append.insert("revision".into(), r.clone());
            }
            agent_operation(ctx, user_id, "appendMessage", &append, can_read, session_id)?;
            let mut document = empty_run_document();
            if let Some(e) = &effort {
                document.insert("effort".into(), json!(e));
            }
            if let Some(m) = mode.filter(|m| m != "default" && !m.is_empty()) {
                document.insert("mode".into(), json!(m));
            }
            if let Some(a) = p
                .get("attachments")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty())
            {
                document.insert("attachments".into(), Value::Array(a.clone()));
            }
            let created = AgentRun {
                id: run_id,
                thread_id: current.id.clone(),
                owner_id: user_id.into(),
                session_id: session_id.into(),
                message_id,
                model,
                lease: String::new(),
                status: "queued".into(),
                document: crate::stringify(&Value::Object(document)),
                created_at: at,
                modified_at: at,
            };
            ctx.db.agent_runs().insert(created.clone());
            Ok(expose_run(&created))
        }
        "steerRun" => {
            // A follow-up delivered into the running turn is saved as an ordinary user message.
            keys(p, &["threadId", "runId", "id", "content"])?;
            let run = ctx.db.agent_runs().id().find(id(p.get("runId"))?);
            let message_id = id(p.get("id"))?;
            let content = text(p.get("content"), MESSAGE_BYTES)?;
            let Some(run) = run.filter(|r| r.owner_id == user_id && r.thread_id == current.id)
            else {
                return fail("notFound");
            };
            if let Some(old) = ctx.db.agent_messages().id().find(message_id.clone()) {
                if old.owner_id != user_id || old.thread_id != current.id || old.content != content
                {
                    return fail("conflict");
                }
                return Ok(json!({ "thread": row(&current), "message": message_json(&old) }));
            }
            if !active_run(&run.status) {
                return fail("conflict");
            }
            if owner_messages() >= MESSAGES - 1 {
                return fail("quotaExceeded");
            }
            let updated = AgentThread {
                revision: current.revision + 1,
                modified_at: at,
                ..current.clone()
            };
            let message = AgentMessage {
                id: message_id,
                owner_id: user_id.into(),
                thread_id: current.id.clone(),
                sequence: updated.revision,
                role: "user".into(),
                content,
                created_at: at,
            };
            ctx.db.agent_messages().insert(message.clone());
            ctx.db.agent_threads().id().update(updated.clone());
            Ok(json!({ "thread": row(&updated), "message": message_json(&message) }))
        }
        "forkThread" => {
            // Copies messages up to a sequence into a new thread. Its first run starts a new provider thread.
            keys(p, &["threadId", "id", "sequence"])?;
            let fork_id = id(p.get("id"))?;
            let Some(sequence) = p
                .get("sequence")
                .and_then(Value::as_f64)
                .filter(|s| s.fract() == 0.0 && *s >= 2.0 && *s <= current.revision as f64)
            else {
                return fail("invalidInput");
            };
            if let Some(old) = ctx.db.agent_threads().id().find(fork_id.clone()) {
                if old.owner_id != user_id {
                    return fail("conflict");
                }
                return Ok(row(&old));
            }
            let mut copied: Vec<AgentMessage> = ctx
                .db
                .agent_messages()
                .thread_id()
                .filter(current.id.as_str())
                .filter(|m| m.sequence as f64 <= sequence)
                .collect();
            copied.sort_by(|a, b| a.sequence.cmp(&b.sequence));
            if ctx.db.agent_threads().owner_id().filter(user_id).count() >= THREADS {
                return fail("quotaExceeded");
            }
            if owner_messages() + copied.len() > MESSAGES {
                return fail("quotaExceeded");
            }
            let title: String = format!("Fork: {}", current.title)
                .chars()
                .take(60)
                .collect();
            let created = AgentThread {
                id: fork_id.clone(),
                owner_id: user_id.into(),
                project_id: current.project_id.clone(),
                title: text(Some(&json!(title)), 200)?.trim().to_owned(),
                revision: copied.len() as u32 + 1,
                created_at: at,
                modified_at: at,
            };
            ctx.db.agent_threads().insert(created.clone());
            for (index, message) in copied.into_iter().enumerate() {
                let new_id = new_uuid(ctx, &|x| {
                    ctx.db.agent_messages().id().find(x.to_owned()).is_some()
                });
                ctx.db.agent_messages().insert(AgentMessage {
                    id: new_id,
                    thread_id: fork_id.clone(),
                    sequence: index as u32 + 2,
                    ..message
                });
            }
            Ok(row(&created))
        }
        "threadShares" => {
            keys(p, &["threadId"])?;
            let items: Vec<Value> = ctx
                .db
                .agent_shares()
                .thread_id()
                .filter(current.id.as_str())
                .filter_map(|s| ctx.db.accounts().id().find(s.grantee_id.clone()).map(|a| json!({ "id": s.id, "username": a.username, "displayName": a.display_name, "createdAt": s.created_at })))
                .collect();
            Ok(json!({ "items": items }))
        }
        "shareThread" => {
            keys(p, &["threadId", "id", "username"])?;
            let share_id = id(p.get("id"))?;
            let username = text(p.get("username"), 100)?.trim().to_lowercase();
            let Some(account) = ctx
                .db
                .accounts()
                .username()
                .find(username)
                .filter(|a| a.enabled)
            else {
                return fail("notFound");
            };
            if account.id == user_id {
                return fail("invalidInput");
            }
            let existing: Vec<AgentShare> = ctx
                .db
                .agent_shares()
                .thread_id()
                .filter(current.id.as_str())
                .collect();
            if let Some(d) = existing.iter().find(|s| s.grantee_id == account.id) {
                return Ok(
                    json!({ "id": d.id, "username": account.username, "displayName": account.display_name, "createdAt": d.created_at }),
                );
            }
            if ctx.db.agent_shares().id().find(share_id.clone()).is_some() {
                return fail("conflict");
            }
            if existing.len() >= 20 {
                return fail("quotaExceeded");
            }
            ctx.db.agent_shares().insert(AgentShare {
                id: share_id.clone(),
                thread_id: current.id.clone(),
                owner_id: user_id.into(),
                grantee_id: account.id.clone(),
                created_at: at,
            });
            Ok(
                json!({ "id": share_id, "username": account.username, "displayName": account.display_name, "createdAt": at }),
            )
        }
        "unshareThread" => {
            keys(p, &["threadId", "id"])?;
            let share = ctx.db.agent_shares().id().find(id(p.get("id"))?);
            let Some(share) = share.filter(|s| s.thread_id == current.id && s.owner_id == user_id)
            else {
                return fail("notFound");
            };
            ctx.db.agent_shares().id().delete(share.id.clone());
            Ok(json!({ "id": share.id, "deleted": true }))
        }
        "renameThread" => {
            keys(p, &["threadId", "title"])?;
            // Titles are presentation only; message revisions and ordering stay unchanged.
            let updated = AgentThread {
                title: text(p.get("title"), 200)?.trim().to_owned(),
                ..current
            };
            ctx.db.agent_threads().id().update(updated.clone());
            Ok(row(&updated))
        }
        "deleteThread" => {
            keys(p, &["threadId"])?;
            let runs: Vec<AgentRun> = ctx
                .db
                .agent_runs()
                .thread_id()
                .filter(current.id.as_str())
                .collect();
            if runs.iter().any(|r| active_run(&r.status)) {
                return fail("conflict");
            }
            for r in runs {
                ctx.db.agent_runs().id().delete(r.id);
            }
            for s in ctx
                .db
                .agent_shares()
                .thread_id()
                .filter(current.id.as_str())
                .collect::<Vec<_>>()
            {
                ctx.db.agent_shares().id().delete(s.id);
            }
            for m in ctx
                .db
                .agent_messages()
                .thread_id()
                .filter(current.id.as_str())
                .collect::<Vec<_>>()
            {
                ctx.db.agent_messages().id().delete(m.id);
            }
            ctx.db.agent_threads().id().delete(current.id.clone());
            Ok(json!({ "id": current.id, "deleted": true }))
        }
        "history" => {
            keys(p, &["threadId", "before"])?;
            let before = before_of(p)?;
            let mut rows: Vec<AgentMessage> = ctx
                .db
                .agent_messages()
                .thread_id()
                .filter(current.id.as_str())
                .filter(|m| (m.sequence as f64) < before)
                .collect();
            rows.sort_by(|a, b| b.sequence.cmp(&a.sequence));
            let mut items: Vec<AgentMessage> = rows.iter().take(PAGE).cloned().collect();
            items.reverse();
            let next = if rows.len() > PAGE {
                json!(items[0].sequence)
            } else {
                Value::Null
            };
            let mut runs: Vec<AgentRun> = ctx
                .db
                .agent_runs()
                .thread_id()
                .filter(current.id.as_str())
                .collect();
            runs.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
            Ok(
                json!({ "thread": row(&current), "items": items.iter().map(message_json).collect::<Vec<_>>(), "nextBefore": next, "runs": runs.iter().map(run_summary).collect::<Vec<_>>() }),
            )
        }
        "appendMessage" => {
            keys(p, &["threadId", "id", "content", "revision"])?;
            let message_id = id(p.get("id"))?;
            let content = text(p.get("content"), MESSAGE_BYTES)?;
            // Repeating an acknowledged append returns its original result without duplication.
            if let Some(old) = ctx.db.agent_messages().id().find(message_id.clone()) {
                if old.owner_id != user_id || old.thread_id != current.id || old.content != content
                {
                    return fail("conflict");
                }
                return Ok(json!({ "thread": row(&current), "message": message_json(&old) }));
            }
            if ctx
                .db
                .agent_runs()
                .thread_id()
                .filter(current.id.as_str())
                .any(|r| active_run(&r.status))
            {
                return fail("conflict");
            }
            if p.get("revision").and_then(Value::as_f64) != Some(current.revision as f64) {
                return fail("conflict");
            }
            if owner_messages() >= MESSAGES {
                return fail("quotaExceeded");
            }
            // Fifty Unicode scalars fit the title's 200-byte storage limit.
            let title = if current.revision == 1 && current.title == "New thread" {
                content
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(50)
                    .collect()
            } else {
                current.title.clone()
            };
            let updated = AgentThread {
                title,
                revision: current.revision + 1,
                modified_at: at,
                ..current.clone()
            };
            let message = AgentMessage {
                id: message_id,
                owner_id: user_id.into(),
                thread_id: current.id.clone(),
                sequence: updated.revision,
                role: "user".into(),
                content,
                created_at: at,
            };
            ctx.db.agent_messages().insert(message.clone());
            ctx.db.agent_threads().id().update(updated.clone());
            Ok(json!({ "thread": row(&updated), "message": message_json(&message) }))
        }
        _ => fail("invalidInput"),
    }
}

fn before_of(p: &Doc) -> Res<f64> {
    match p.get("before") {
        None | Some(Value::Null) => Ok(4_294_967_295.0),
        Some(v) => match v.as_f64() {
            Some(f) if f.fract() == 0.0 && (1.0..=4_294_967_295.0).contains(&f) => Ok(f),
            _ => fail("invalidInput"),
        },
    }
}

/// The service-only executor's steps: claim, check, progress, finish, interrupt; `startup`
/// marks runs a restart left behind.
pub fn agent_worker(
    ctx: &ReducerContext,
    run_id: &str,
    lease: &str,
    operation: &str,
    document: &str,
) -> Res<Value> {
    let at = now(ctx);
    let with_error = |r: &AgentRun, status: &str, error: &str| -> AgentRun {
        let mut d: Value = serde_json::from_str(&r.document).unwrap_or(json!({}));
        d["error"] = json!(error);
        AgentRun {
            status: status.into(),
            lease: String::new(),
            modified_at: at,
            document: crate::stringify(&d),
            ..r.clone()
        }
    };
    if operation == "startup" {
        for r in ctx.db.agent_runs().iter().collect::<Vec<_>>() {
            if active_run(&r.status) {
                ctx.db.agent_runs().id().update(with_error(&r, "interrupted", "Dar restarted before this run completed. Inspect its output before starting another run."));
            }
        }
        return Ok(json!({ "ok": true }));
    }
    let Some(r) = ctx.db.agent_runs().id().find(run_id.to_owned()) else {
        return fail("notFound");
    };
    let authority = || -> Res<()> {
        let owner = crate::from_session(ctx, &r.session_id).map(|u| u.id);
        if !app_enabled(ctx, "agent") || owner.as_deref() != Ok(r.owner_id.as_str()) {
            return fail("forbidden");
        }
        let project = ctx
            .db
            .agent_threads()
            .id()
            .find(r.thread_id.clone())
            .and_then(|t| ctx.db.agent_projects().id().find(t.project_id));
        let Some(project) = project.filter(|x| x.owner_id == r.owner_id) else {
            return fail("forbidden");
        };
        if project.source_type == "files" {
            let folder = ctx.db.file_entries().id().find(project.source_ref.clone());
            if folder.is_none_or(|f| {
                f.kind != "folder" || access(ctx, &f, &r.owner_id, Unlock::Caller).is_none()
            }) {
                return fail("forbidden");
            }
        }
        Ok(())
    };
    if operation == "claim" {
        if r.status != "queued" {
            return Ok(Value::Null);
        }
        authority()?;
        let next = AgentRun {
            status: "running".into(),
            lease: lease.into(),
            modified_at: at,
            ..r.clone()
        };
        ctx.db.agent_runs().id().update(next.clone());
        return Ok(row(&next));
    }
    if (r.lease.is_empty() || r.lease != lease)
        && !(operation == "interrupt" && r.status == "queued")
        || !active_run(&r.status)
    {
        return fail("conflict");
    }
    if operation == "check" {
        if r.status == "cancelling" {
            return fail("forbidden");
        }
        authority()?;
        return Ok(json!({ "ok": true }));
    }
    if operation == "interrupt" {
        let cancelled = r.status == "cancelling";
        ctx.db.agent_runs().id().update(with_error(
            &r,
            if cancelled {
                "cancelled"
            } else {
                "interrupted"
            },
            if cancelled {
                "Run cancelled."
            } else {
                "Execution stopped. Inspect saved output before retrying."
            },
        ));
        return Ok(json!({ "ok": true }));
    }
    if r.status == "cancelling" {
        return fail("forbidden");
    }
    authority()?;
    if document.encode_utf16().count() > 512 * 1024 {
        return fail("operationTooLarge");
    }
    let input: Value = serde_json::from_str(document).map_err(|_| "invalidInput".to_owned())?;
    let data = validate_run_document(&input["document"])?;
    let status = if operation == "finish" {
        crate::text_of(input.get("status"))
    } else {
        "running".to_owned()
    };
    if !["running", "completed", "failed", "cancelled"].contains(&status.as_str()) {
        return fail("invalidInput");
    }
    ctx.db.agent_runs().id().update(AgentRun {
        status: status.clone(),
        document: crate::stringify(&data),
        lease: if operation == "finish" {
            String::new()
        } else {
            lease.into()
        },
        modified_at: at,
        ..r.clone()
    });
    let output = data["output"].as_str().unwrap_or_default();
    if operation == "finish" && status == "completed" && !output.is_empty() {
        let t = ctx
            .db
            .agent_threads()
            .id()
            .find(r.thread_id.clone())
            .expect("thread");
        if ctx
            .db
            .agent_messages()
            .owner_id()
            .filter(r.owner_id.as_str())
            .count()
            >= MESSAGES
        {
            return fail("quotaExceeded");
        }
        let message_id = new_uuid(ctx, &|x| {
            ctx.db.agent_messages().id().find(x.to_owned()).is_some()
        });
        // Full bounded output is retained on the run; the message fits the history contract.
        let mut content = String::new();
        let mut bytes = 0;
        for character in output.chars() {
            bytes += character.len_utf8();
            if bytes > 16000 {
                break;
            }
            content.push(character);
        }
        if content != output {
            content.push_str("\n\n[Full response is in the run output.]");
        }
        ctx.db.agent_messages().insert(AgentMessage {
            id: message_id,
            owner_id: r.owner_id.clone(),
            thread_id: t.id.clone(),
            sequence: t.revision + 1,
            role: "assistant".into(),
            content,
            created_at: at,
        });
        ctx.db.agent_threads().id().update(AgentThread {
            revision: t.revision + 1,
            modified_at: at,
            ..t
        });
    }
    Ok(json!({ "ok": true }))
}

pub fn agent_snapshot(ctx: &ReducerContext) -> Value {
    json!({
        "projects": ctx.db.agent_projects().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "threads": ctx.db.agent_threads().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "messages": ctx.db.agent_messages().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "runs": ctx.db.agent_runs().iter().map(|r| { let mut v = row(&r); let o = v.as_object_mut().expect("row"); o.remove("lease"); o.remove("sessionId"); v }).collect::<Vec<_>>(),
        "shares": ctx.db.agent_shares().iter().map(|r| row(&r)).collect::<Vec<_>>(),
    })
}

/// V1–V5 backups have no Agent data. V6 validates links and bounded account data.
pub fn restore_agent(ctx: &ReducerContext, value: Option<&Value>) -> Res<()> {
    let Some(value) = value else { return Ok(()) };
    if !value["projects"].is_array()
        || !value["threads"].is_array()
        || !value["messages"].is_array()
    {
        return fail("invalidInput");
    }
    let parse = |v: &Value| -> Res<Value> { Ok(v.clone()) };
    for v in value["projects"].as_array().expect("array") {
        let r: AgentProject =
            serde_json::from_value(parse(v)?).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.accounts().id().find(r.owner_id.clone()).is_none()
            || crate::opaque_id(&r.id).is_err()
            || text(Some(&json!(r.name)), 160)?.trim() != r.name
            || !r.created_at.is_finite()
            || ctx
                .db
                .agent_projects()
                .owner_id()
                .filter(r.owner_id.as_str())
                .count()
                >= PROJECTS
        {
            return fail("invalidInput");
        }
        match r.source_type.as_str() {
            "git" => {
                if !valid_repository_url(&text(Some(&json!(r.source_ref)), 2048)?) {
                    return fail("invalidInput");
                }
            }
            "files" => crate::opaque_id(&r.source_ref)?,
            "scratch" => {
                if r.source_ref != "scratch" {
                    return fail("invalidInput");
                }
            }
            _ => return fail("invalidInput"),
        }
        ctx.db.agent_projects().insert(r);
    }
    let threads: Vec<AgentThread> =
        serde_json::from_value(value["threads"].clone()).map_err(|_| "invalidInput".to_owned())?;
    for r in &threads {
        let parent = ctx.db.agent_projects().id().find(r.project_id.clone());
        if parent.is_none_or(|x| x.owner_id != r.owner_id)
            || crate::opaque_id(&r.id).is_err()
            || text(Some(&json!(r.title)), 200)?.trim() != r.title
            || r.revision < 1
            || r.revision as usize > MESSAGES + 1
            || !r.created_at.is_finite()
            || !r.modified_at.is_finite()
            || ctx
                .db
                .agent_threads()
                .owner_id()
                .filter(r.owner_id.as_str())
                .count()
                >= THREADS
        {
            return fail("invalidInput");
        }
        ctx.db.agent_threads().insert(r.clone());
    }
    let mut sequences = std::collections::HashSet::new();
    for v in value["messages"].as_array().expect("array") {
        let r: AgentMessage =
            serde_json::from_value(v.clone()).map_err(|_| "invalidInput".to_owned())?;
        let parent = ctx.db.agent_threads().id().find(r.thread_id.clone());
        let key = (r.thread_id.clone(), r.sequence);
        if parent
            .as_ref()
            .is_none_or(|x| x.owner_id != r.owner_id || r.sequence > x.revision)
            || crate::opaque_id(&r.id).is_err()
            || !["user", "assistant"].contains(&r.role.as_str())
            || r.sequence < 2
            || sequences.contains(&key)
            || !r.created_at.is_finite()
            || ctx
                .db
                .agent_messages()
                .owner_id()
                .filter(r.owner_id.as_str())
                .count()
                >= MESSAGES
        {
            return fail("invalidInput");
        }
        text(Some(&json!(r.content)), MESSAGE_BYTES)?;
        sequences.insert(key);
        ctx.db.agent_messages().insert(r);
    }
    for t in &threads {
        if ctx
            .db
            .agent_messages()
            .thread_id()
            .filter(t.id.as_str())
            .count()
            != t.revision as usize - 1
        {
            return fail("invalidInput");
        }
    }
    for v in value["runs"].as_array().cloned().unwrap_or_default() {
        let mut v = v;
        v["lease"] = json!("");
        v["sessionId"] = json!("");
        let r: AgentRun = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        let parent = ctx.db.agent_threads().id().find(r.thread_id.clone());
        let message = ctx.db.agent_messages().id().find(r.message_id.clone());
        let ok = parent.as_ref().is_some_and(|x| x.owner_id == r.owner_id)
            && message.is_some_and(|m| {
                Some(m.thread_id.clone()) == parent.as_ref().map(|x| x.id.clone())
                    && m.role == "user"
            })
            && crate::opaque_id(&r.id).is_ok()
            && [
                "queued",
                "running",
                "cancelling",
                "completed",
                "failed",
                "cancelled",
                "interrupted",
            ]
            .contains(&r.status.as_str())
            && r.document.encode_utf16().count() <= 512 * 1024
            && ctx
                .db
                .agent_runs()
                .owner_id()
                .filter(r.owner_id.as_str())
                .count()
                < 100
            && r.created_at.is_finite()
            && r.modified_at.is_finite()
            && !r.model.is_empty()
            && r.model.encode_utf16().count() <= 120;
        if !ok {
            return fail("invalidInput");
        }
        validate_run_document(
            &serde_json::from_str(&r.document).map_err(|_| "invalidInput".to_owned())?,
        )?;
        let status = if active_run(&r.status) {
            "interrupted".to_owned()
        } else {
            r.status.clone()
        };
        ctx.db.agent_runs().insert(AgentRun { status, ..r });
    }
    // Older V6 backups have no shares; each grant must point at a restored thread and account.
    for v in value["shares"].as_array().cloned().unwrap_or_default() {
        let r: AgentShare = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        let parent = ctx.db.agent_threads().id().find(r.thread_id.clone());
        if parent.is_none_or(|x| x.owner_id != r.owner_id)
            || ctx.db.accounts().id().find(r.grantee_id.clone()).is_none()
            || r.grantee_id == r.owner_id
            || crate::opaque_id(&r.id).is_err()
            || !r.created_at.is_finite()
            || ctx
                .db
                .agent_shares()
                .thread_id()
                .filter(r.thread_id.as_str())
                .count()
                >= 20
        {
            return fail("invalidInput");
        }
        ctx.db.agent_shares().insert(r);
    }
    Ok(())
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_agent(
    ctx: &mut ProcedureContext,
    sessionId: String,
    operation: String,
    document: String,
) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let user = crate::from_session(tx, &sessionId)?;
        if tx
            .db
            .app_passwords()
            .session_id()
            .find(sessionId.clone())
            .is_some()
        {
            return fail("forbidden");
        }
        if document.encode_utf16().count() > 128 * 1024 {
            return fail("operationTooLarge");
        }
        let value = crate::document(&document)?;
        let can_read = |entry: &FileEntry| access(tx, entry, &user.id, Unlock::Caller).is_some();
        agent_operation(tx, &user.id, &operation, &value, &can_read, &sessionId)
    })
}

#[spacetimedb::procedure]
pub fn service_agent_worker(
    ctx: &mut ProcedureContext,
    id: String,
    lease: String,
    operation: String,
    document: String,
) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        agent_worker(tx, &id, &lease, &operation, &document)
    })
}
