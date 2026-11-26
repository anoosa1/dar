//! T3-parity Agent surfaces and reusable workflow panels:
//! - Multi-surface right panel switcher (`Diff`, `Files`, `Side chat`, `PRs`, `Preview`)
//! - Side-by-side Split Diff viewer with inline line-level review comments attached to composer
//! - Working-copy File Explorer & Viewer (`GET /api/v1/agent/threads/:id/file`)
//! - Parallel Side Chat pane
//! - Dar Git PRs & Issues surface
//! - Sandboxed HTML/SVG/Markdown Preview surface
//! - Project Scripts manager inside the Floating Details Card

use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{self, Req};
use crate::git::parts::{API as GIT_API, GitIssueList, GitIssueSummary, GitRepository, repo_api};
use crate::icons::Icon;
use crate::markdown::Markdown;
use crate::random_uuid;

const AGENT_BASE: &str = "/api/v1/agent";

/// Surfaces available inside the tabbed right-hand Agent panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    Diff,
    Files,
    SideChat,
    PullRequests,
    Preview,
}

impl SurfaceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Diff => "Changes",
            Self::Files => "Files",
            Self::SideChat => "Side chat",
            Self::PullRequests => "PRs & Issues",
            Self::Preview => "Preview",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Diff => "diff",
            Self::Files => "folder",
            Self::SideChat => "chat",
            Self::PullRequests => "git",
            Self::Preview => "globe",
        }
    }

    pub fn all() -> [Self; 5] {
        [
            Self::Diff,
            Self::Files,
            Self::SideChat,
            Self::PullRequests,
            Self::Preview,
        ]
    }
}

/// Inline context chip attached to the composer (file mention, diff comment, terminal snippet, quote).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextChip {
    pub id: String,
    pub label: String,
    pub kind: &'static str, // "file" | "diff" | "terminal" | "quote" | "pr"
    pub content: String,
}

/// Saved project script (`agentProjectScripts[projectId]`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScript {
    pub id: String,
    pub name: String,
    pub command: String,
}

/// Saved prompt stash entry (`agentPromptStash`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptStashEntry {
    pub id: String,
    pub text: String,
    pub created_at: f64,
}

/// Saved automation / scheduled workflow entry (`agentSchedules`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentScheduleEntry {
    pub id: String,
    pub title: String,
    pub project_id: String,
    pub prompt: String,
    pub cadence: String,
    pub enabled: bool,
}

/// Parsed split-diff row pairing old (left) and new (right) lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitRow {
    pub hunk: Option<String>,
    pub left_num: Option<usize>,
    pub left_text: String,
    pub left_kind: &'static str, // "context" | "removed" | "empty"
    pub right_num: Option<usize>,
    pub right_text: String,
    pub right_kind: &'static str, // "context" | "added" | "empty"
}

fn parse_hunk_start(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let old_n = rest.bytes().take_while(u8::is_ascii_digit).count();
    if old_n == 0 {
        return None;
    }
    let old_line: usize = rest[..old_n].parse().ok()?;
    let mut after = &rest[old_n..];
    if let Some(comma) = after.strip_prefix(',') {
        let d = comma.bytes().take_while(u8::is_ascii_digit).count();
        after = &comma[d..];
    }
    let plus = after.strip_prefix(" +")?;
    let new_n = plus.bytes().take_while(u8::is_ascii_digit).count();
    if new_n == 0 {
        return None;
    }
    let new_line: usize = plus[..new_n].parse().ok()?;
    Some((old_line, new_line))
}

pub fn build_split_rows(patch: &str) -> Vec<SplitRow> {
    let mut rows = Vec::new();
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut in_hunk = false;
    let mut removed_buf: Vec<(usize, String)> = Vec::new();
    let mut added_buf: Vec<(usize, String)> = Vec::new();

    let flush = |rows: &mut Vec<SplitRow>,
                 removed: &mut Vec<(usize, String)>,
                 added: &mut Vec<(usize, String)>| {
        let n = removed.len().max(added.len());
        for i in 0..n {
            let l = removed.get(i);
            let r = added.get(i);
            rows.push(SplitRow {
                hunk: None,
                left_num: l.map(|(num, _)| *num),
                left_text: l.map(|(_, t)| t.clone()).unwrap_or_default(),
                left_kind: if l.is_some() { "removed" } else { "empty" },
                right_num: r.map(|(num, _)| *num),
                right_text: r.map(|(_, t)| t.clone()).unwrap_or_default(),
                right_kind: if r.is_some() { "added" } else { "empty" },
            });
        }
        removed.clear();
        added.clear();
    };

    for line in patch.split('\n') {
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            continue;
        }
        if line.starts_with("@@") {
            flush(&mut rows, &mut removed_buf, &mut added_buf);
            if let Some((o, n)) = parse_hunk_start(line) {
                old_line = o;
                new_line = n;
            }
            in_hunk = true;
            rows.push(SplitRow {
                hunk: Some(line.to_owned()),
                left_num: None,
                left_text: String::new(),
                left_kind: "empty",
                right_num: None,
                right_text: String::new(),
                right_kind: "empty",
            });
            continue;
        }
        if !in_hunk {
            continue;
        }
        if let Some(rest) = line.strip_prefix('-') {
            removed_buf.push((old_line, rest.to_owned()));
            old_line += 1;
        } else if let Some(rest) = line.strip_prefix('+') {
            added_buf.push((new_line, rest.to_owned()));
            new_line += 1;
        } else {
            flush(&mut rows, &mut removed_buf, &mut added_buf);
            let rest = line.strip_prefix(' ').unwrap_or(line);
            rows.push(SplitRow {
                hunk: None,
                left_num: Some(old_line),
                left_text: rest.to_owned(),
                left_kind: "context",
                right_num: Some(new_line),
                right_text: rest.to_owned(),
                right_kind: "context",
            });
            old_line += 1;
            new_line += 1;
        }
    }
    flush(&mut rows, &mut removed_buf, &mut added_buf);
    rows
}

/// Side-by-side Split Diff viewer with clickable inline review comments.
#[component]
pub fn SplitDiffView(
    #[prop(into)] path: String,
    #[prop(into)] patch: String,
    #[prop(into)] wrap: Signal<bool>,
    on_comment: Callback<ContextChip>,
) -> impl IntoView {
    let rows = StoredValue::new(build_split_rows(&patch));
    let path_sv = StoredValue::new(path);
    let commenting_line: RwSignal<Option<(usize, String)>> = RwSignal::new(None);
    let comment_text = RwSignal::new(String::new());

    view! {
        <div class=move || if wrap.get() { "agent-split-diff is-wrap" } else { "agent-split-diff" } role="region" aria-label=format!("Split diff for {}", path_sv.get_value())>
            {rows.get_value().into_iter().map(|row| {
                if let Some(hunk) = row.hunk {
                    return view! { <div class="agent-split-hunk">{hunk}</div> }.into_any();
                }
                let line_num = row.right_num.or(row.left_num).unwrap_or(1);
                let line_preview = if !row.right_text.is_empty() {
                    row.right_text.clone()
                } else {
                    row.left_text.clone()
                };
                let preview_for_btn = line_preview.clone();
                view! {
                    <div class="agent-split-row">
                        <div class=format!("agent-split-cell is-{}", row.left_kind)>
                            <span class="agent-split-num">{row.left_num.map(|n| n.to_string()).unwrap_or_default()}</span>
                            <span class="agent-split-code">{if row.left_text.is_empty() { " ".to_owned() } else { row.left_text }}</span>
                        </div>
                        <div class=format!("agent-split-cell is-{}", row.right_kind)>
                            <span class="agent-split-num">
                                {row.right_num.map(|n| n.to_string()).unwrap_or_default()}
                                <button type="button" class="agent-split-comment-btn" title=format!("Comment on line {line_num}") aria-label=format!("Comment on line {line_num}")
                                    on:click=move |_| {
                                        comment_text.set(String::new());
                                        commenting_line.set(Some((line_num, preview_for_btn.clone())));
                                    }>"+"</button>
                            </span>
                            <span class="agent-split-code">{if row.right_text.is_empty() { " ".to_owned() } else { row.right_text }}</span>
                        </div>
                    </div>
                }.into_any()
            }).collect_view()}
            {move || commenting_line.get().map(|(line_num, snippet)| {
                let path_cur = path_sv.get_value();
                let snippet_submit = snippet.clone();
                view! {
                    <form class="agent-inline-comment" on:submit=move |e| {
                        e.prevent_default();
                        let note = comment_text.get_untracked().trim().to_owned();
                        if note.is_empty() {
                            return;
                        }
                        on_comment.try_run(ContextChip {
                            id: random_uuid(),
                            label: format!("{path_cur}:{line_num}"),
                            kind: "diff",
                            content: format!("Review note on `{path_cur}:{line_num}` (`{}`): {note}", snippet_submit.trim()),
                        });
                        commenting_line.set(None);
                        comment_text.set(String::new());
                    }>
                        <div class="agent-inline-comment-head">
                            <strong>{format!("Comment on {path_cur}:{line_num}")}</strong>
                            <button type="button" class="icon-button" aria-label="Cancel comment" on:click=move |_| commenting_line.set(None)><Icon name="close"/></button>
                        </div>
                        <input type="text" placeholder="Describe the change or question for the agent\u{2026}" autofocus
                            prop:value=move || comment_text.get()
                            on:input=move |e| comment_text.set(event_target_value(&e))/>
                        <div class="agent-inline-comment-actions">
                            <button type="button" class="secondary-button" on:click=move |_| commenting_line.set(None)>"Cancel"</button>
                            <button type="submit" class="primary-button" disabled=move || comment_text.with(|s| s.trim().is_empty())>"Attach to composer"</button>
                        </div>
                    </form>
                }
            })}
        </div>
    }
}

#[derive(Clone, Debug, Deserialize)]
struct FilesListResp {
    #[serde(default)]
    source: String,
    #[serde(default)]
    items: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFileContent {
    pub path: String,
    pub media_type: String,
    pub size: u64,
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub content: String,
}

fn encode_uri(value: &str) -> String {
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_default()
}

/// Working-copy Files surface: browse project/working-copy files and inspect their contents.
#[component]
pub fn FilesSurface(
    #[prop(into)] thread_id: Signal<String>,
    #[prop(into)] initial_file: Signal<Option<String>>,
    on_attach_chip: Callback<ContextChip>,
    on_open_preview: Callback<String>,
) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let files: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let source = RwSignal::new(String::new());
    let loading_list = RwSignal::new(false);
    let list_error = RwSignal::new(String::new());

    let selected_path = RwSignal::new(String::new());
    let loaded_file: RwSignal<Option<WorkspaceFileContent>> = RwSignal::new(None);
    let loading_file = RwSignal::new(false);
    let file_error = RwSignal::new(String::new());
    let render_md = RwSignal::new(true);

    // Sync externally requested file path (e.g. clicking a file in WorkLog).
    Effect::new(move |_| {
        if let Some(p) = initial_file.get()
            && !p.is_empty()
        {
            selected_path.set(p);
        }
    });

    // Fetch file list whenever thread or query changes.
    Effect::new(move |_| {
        let tid = thread_id.get();
        let q = query.get();
        if tid.is_empty() {
            files.set(Vec::new());
            return;
        }
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        loading_list.set(true);
        list_error.set(String::new());
        spawn_local(async move {
            let url = format!("{AGENT_BASE}/threads/{tid}/files?query={}", encode_uri(&q));
            match api::request::<FilesListResp>(
                &url,
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(res) if !signal.aborted() => {
                    source.try_set(res.source);
                    files.try_set(res.items);
                }
                Err(e) if !signal.aborted() => {
                    list_error.try_set(e.message);
                }
                _ => {}
            }
            if !signal.aborted() {
                loading_list.try_set(false);
            }
        });
    });

    // Fetch selected file content.
    Effect::new(move |_| {
        let tid = thread_id.get();
        let path = selected_path.get();
        if tid.is_empty() || path.is_empty() {
            loaded_file.set(None);
            return;
        }
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        loading_file.set(true);
        file_error.set(String::new());
        spawn_local(async move {
            let url = format!("{AGENT_BASE}/threads/{tid}/file?path={}", encode_uri(&path));
            match api::request::<WorkspaceFileContent>(
                &url,
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(file) if !signal.aborted() => {
                    loaded_file.try_set(Some(file));
                }
                Err(e) if !signal.aborted() => {
                    file_error.try_set(if e.message.is_empty() {
                        "Could not read file.".into()
                    } else {
                        e.message
                    });
                    loaded_file.try_set(None);
                }
                _ => {}
            }
            if !signal.aborted() {
                loading_file.try_set(false);
            }
        });
    });

    view! {
        <div class="agent-files-surface">
            <div class="agent-surface-search">
                <Icon name="search"/>
                <input type="search" aria-label="Filter workspace files" placeholder="Filter workspace files\u{2026}"
                    prop:value=move || query.get()
                    on:input=move |e| query.set(event_target_value(&e))/>
                {move || (!selected_path.with(String::is_empty)).then(|| view! {
                    <button type="button" class="text-button" on:click=move |_| selected_path.set(String::new())>"All files"</button>
                })}
            </div>
            {move || {
                let err = list_error.get();
                (!err.is_empty()).then(|| view! { <p class="error agent-review-note" role="alert">{err}</p> })
            }}
            {move || {
                let sel = selected_path.get();
                if sel.is_empty() {
                    let list = files.get();
                    let src = source.get();
                    if loading_list.get() && list.is_empty() {
                        return view! { <p class="agent-review-note" role="status">"Loading files\u{2026}"</p> }.into_any();
                    }
                    if list.is_empty() {
                        return view! {
                            <div class="agent-review-empty">
                                <Icon name="folder"/>
                                <h3>"No workspace files"</h3>
                                <p>{if src == "none" { "Run the agent once to materialize this thread\u{2019}s working copy." } else { "No files match your filter." }}</p>
                            </div>
                        }.into_any();
                    }
                    view! {
                        <ul class="agent-file-tree" aria-label="Workspace files">
                            {list.into_iter().map(|path| {
                                let p_click = path.clone();
                                let p_attach = path.clone();
                                let p_title = path.clone();
                                view! {
                                    <li class="agent-file-tree-row">
                                        <button type="button" class="agent-file-tree-open" on:click=move |_| selected_path.set(p_click.clone())>
                                            <Icon name="file"/>
                                            <span title=p_title>{path}</span>
                                        </button>
                                        <button type="button" class="icon-button" title="Mention in composer" aria-label="Mention in composer"
                                            on:click=move |_| {
                                                on_attach_chip.try_run(ContextChip {
                                                    id: random_uuid(),
                                                    label: format!("@{p_attach}"),
                                                    kind: "file",
                                                    content: format!("@{p_attach}"),
                                                });
                                            }>
                                            <Icon name="plus"/>
                                        </button>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any()
                } else {
                    if loading_file.get() {
                        return view! { <p class="agent-review-note" role="status">"Loading file\u{2026}"</p> }.into_any();
                    }
                    let err = file_error.get();
                    if !err.is_empty() {
                        return view! { <p class="error agent-review-note" role="alert">{err}</p> }.into_any();
                    }
                    let Some(file) = loaded_file.get() else {
                        return ().into_any();
                    };
                    let is_md = file.media_type == "text/markdown" || file.path.ends_with(".md");
                    let can_preview = is_md || file.media_type == "text/html" || file.media_type == "image/svg+xml";
                    let path_attach = file.path.clone();
                    let path_prev = file.path.clone();
                    let content_attach = file.content.clone();
                    let is_bin = file.binary;
                    view! {
                        <div class="agent-file-viewer">
                            <div class="agent-file-viewer-head">
                                <button type="button" class="icon-button" aria-label="Back to files" on:click=move |_| selected_path.set(String::new())><Icon name="back"/></button>
                                <code title=file.path.clone()>{file.path.clone()}</code>
                                {is_md.then(|| view! {
                                    <button type="button" class="text-button" on:click=move |_| render_md.update(|v| *v = !*v)>
                                        {move || if render_md.get() { "Raw" } else { "Rendered" }}
                                    </button>
                                })}
                                {can_preview.then(|| view! {
                                    <button type="button" class="text-button" on:click=move |_| { on_open_preview.try_run(path_prev.clone()); }>"Preview"</button>
                                })}
                                <button type="button" class="text-button" on:click=move |_| {
                                    let snippet: String = if is_bin {
                                        format!("@{path_attach}")
                                    } else {
                                        let preview: String = content_attach.lines().take(30).collect::<Vec<_>>().join("\n");
                                        format!("File `@{path_attach}`:\n```\n{preview}\n```")
                                    };
                                    on_attach_chip.try_run(ContextChip {
                                        id: random_uuid(),
                                        label: format!("@{path_attach}"),
                                        kind: "file",
                                        content: snippet,
                                    });
                                }>"Attach"</button>
                            </div>
                            {file.truncated.then(|| view! { <p class="agent-review-note">"Showing first 512 KiB of this file."</p> })}
                            {if file.binary {
                                if file.media_type.starts_with("image/") && !file.content.is_empty() {
                                    view! { <div class="agent-file-image"><img src=file.content alt=file.path/></div> }.into_any()
                                } else {
                                    view! { <p class="agent-review-note">{format!("Binary file ({}, {} bytes).", file.media_type, file.size)}</p> }.into_any()
                                }
                            } else if is_md && render_md.get() {
                                view! { <div class="agent-file-markdown"><Markdown source=file.content/></div> }.into_any()
                            } else {
                                let lines: Vec<(usize, String)> = file.content.lines().enumerate().map(|(i, l)| (i + 1, l.to_owned())).collect();
                                view! {
                                    <div class="agent-file-code">
                                        {lines.into_iter().map(|(n, text)| view! {
                                            <div class="agent-file-code-line">
                                                <span class="agent-file-code-num">{n}</span>
                                                <span class="agent-file-code-text">{if text.is_empty() { " ".to_owned() } else { text }}</span>
                                            </div>
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SideNote {
    id: String,
    text: String,
}

/// Parallel Side Chat / Scratchpad surface beside the main conversation.
#[component]
pub fn SideChatSurface(
    #[prop(into)] thread_id: Signal<String>,
    on_attach_chip: Callback<ContextChip>,
    on_send_prompt: Callback<String>,
) -> impl IntoView {
    let notes: RwSignal<Vec<SideNote>> = RwSignal::new(Vec::new());
    let input = RwSignal::new(String::new());

    // Keep side notes clean per thread.
    Effect::new(move |_| {
        let _ = thread_id.get();
        notes.set(Vec::new());
        input.set(String::new());
    });

    let add_note = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        let text = input.get_untracked().trim().to_owned();
        if text.is_empty() {
            return;
        }
        notes.update(|list| {
            list.push(SideNote {
                id: random_uuid(),
                text,
            });
        });
        input.set(String::new());
    };

    view! {
        <div class="agent-sidechat">
            <div class="agent-sidechat-intro">
                <p>"Keep parallel side questions, review notes, or follow-up ideas beside the main thread without cluttering its timeline."</p>
            </div>
            <div class="agent-sidechat-list">
                {move || {
                    let list = notes.get();
                    if list.is_empty() {
                        return view! {
                            <div class="agent-review-empty">
                                <Icon name="chat"/>
                                <h3>"Side chat"</h3>
                                <p>"Jot down questions or edge cases while the agent works, then attach or send them when ready."</p>
                            </div>
                        }.into_any();
                    }
                    list.into_iter().map(|note| {
                        let id_del = note.id.clone();
                        let txt_att = note.text.clone();
                        let txt_send = note.text.clone();
                        view! {
                            <article class="agent-sidechat-card">
                                <p>{note.text}</p>
                                <div class="agent-sidechat-card-actions">
                                    <button type="button" class="text-button" on:click=move |_| {
                                        on_attach_chip.try_run(ContextChip {
                                            id: random_uuid(),
                                            label: "Side note".into(),
                                            kind: "quote",
                                            content: txt_att.clone(),
                                        });
                                    }>"Attach"</button>
                                    <button type="button" class="text-button" on:click=move |_| { on_send_prompt.try_run(txt_send.clone()); }>"Use in composer"</button>
                                    <button type="button" class="icon-button" aria-label="Delete note" on:click=move |_| notes.update(|l| l.retain(|x| x.id != id_del))><Icon name="trash"/></button>
                                </div>
                            </article>
                        }
                    }).collect_view().into_any()
                }}
            </div>
            <form class="agent-sidechat-form" on:submit=add_note>
                <input type="text" aria-label="Side note" placeholder="Write a side question or note\u{2026}"
                    prop:value=move || input.get()
                    on:input=move |e| input.set(event_target_value(&e))/>
                <button type="submit" class="primary-button" disabled=move || input.with(|s| s.trim().is_empty())>"Add"</button>
            </form>
        </div>
    }
}

#[derive(Deserialize)]
struct RepoListResp {
    #[serde(default)]
    items: Vec<GitRepository>,
}

/// Live Dar Git Pull Requests & Issues surface inside the right panel.
#[component]
pub fn PullRequestsSurface(on_attach_chip: Callback<ContextChip>) -> impl IntoView {
    let items: RwSignal<Vec<(String, String, GitIssueSummary)>> = RwSignal::new(Vec::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(String::new());
    let filter_kind = RwSignal::new("all");

    Effect::new(move |_| {
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        loading.set(true);
        spawn_local(async move {
            let repos = match api::request::<RepoListResp>(
                &format!("{GIT_API}/repositories"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(r) => r.items,
                Err(e) => {
                    if !signal.aborted() {
                        error.try_set(e.message);
                        loading.try_set(false);
                    }
                    return;
                }
            };
            let mut collected = Vec::new();
            for repo in repos.into_iter().take(12) {
                if signal.aborted() {
                    return;
                }
                if repo.open_pulls == 0 && repo.open_issues == 0 {
                    continue;
                }
                if let Ok(list) = api::request::<GitIssueList>(
                    &format!("{}/issues?state=open", repo_api(&repo.id)),
                    Req {
                        signal: Some(signal.clone()),
                        ..Default::default()
                    },
                )
                .await
                {
                    for item in list.items {
                        collected.push((repo.id.clone(), repo.name.clone(), item));
                    }
                }
            }
            if !signal.aborted() {
                collected.sort_by(|a, b| b.2.updated_at.total_cmp(&a.2.updated_at));
                items.try_set(collected);
                loading.try_set(false);
            }
        });
    });

    view! {
        <div class="agent-prs-surface">
            <div class="agent-surface-pills" role="tablist" aria-label="Filter items">
                {[("all", "All"), ("pull", "Pull requests"), ("issue", "Issues")].into_iter().map(|(k, lbl)| view! {
                    <button type="button" role="tab" aria-selected=move || (filter_kind.get() == k).to_string() on:click=move |_| filter_kind.set(k)>{lbl}</button>
                }).collect_view()}
            </div>
            {move || {
                let e = error.get();
                if !e.is_empty() {
                    return view! { <p class="error agent-review-note" role="alert">{e}</p> }.into_any();
                }
                if loading.get() {
                    return view! { <p class="agent-review-note" role="status">"Loading Dar Git pull requests & issues\u{2026}"</p> }.into_any();
                }
                let fk = filter_kind.get();
                let shown: Vec<(String, String, GitIssueSummary)> = items
                    .get()
                    .into_iter()
                    .filter(|(_, _, it)| fk == "all" || it.kind == fk)
                    .collect();
                if shown.is_empty() {
                    return view! {
                        <div class="agent-review-empty">
                            <Icon name="git"/>
                            <h3>"No open pull requests or issues"</h3>
                            <p>"Open pull requests and issues in Dar Git repositories appear here automatically."</p>
                            <a href="/git" class="secondary-button">"Open Dar Git"</a>
                        </div>
                    }.into_any();
                }
                view! {
                    <ul class="agent-pr-list">
                        {shown.into_iter().map(|(repo_id, repo_name, item)| {
                            let is_pr = item.kind == "pull";
                            let num = item.number;
                            let title = item.title.clone();
                            let repo_lbl = repo_name.clone();
                            let href = format!("/git?repo={repo_id}&view={}&item={num}", if is_pr { "pulls" } else { "issues" });
                            view! {
                                <li class="agent-pr-row">
                                    <div class="agent-pr-head">
                                        <span class=if is_pr { "agent-pr-badge is-pr" } else { "agent-pr-badge is-issue" }>
                                            {if is_pr { format!("PR #{num}") } else { format!("#{num}") }}
                                        </span>
                                        <span class="agent-pr-repo">{repo_name}</span>
                                    </div>
                                    <strong>{item.title}</strong>
                                    <div class="agent-pr-meta">
                                        <span>{format!("by {}", item.author)}</span>
                                        {item.head.map(|h| view! { <code>{h}</code> })}
                                    </div>
                                    <div class="agent-pr-actions">
                                        <a href=href class="text-button">"Open in Git"</a>
                                        <button type="button" class="text-button" on:click=move |_| {
                                            on_attach_chip.try_run(ContextChip {
                                                id: random_uuid(),
                                                label: format!("{repo_lbl} #{num}"),
                                                kind: "pr",
                                                content: format!("Regarding Dar Git {repo_lbl} #{num} ({title}):"),
                                            });
                                        }>"Discuss"</button>
                                    </div>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                }.into_any()
            }}
        </div>
    }
}

/// Sandboxed Preview surface for HTML, SVG, and Markdown workspace files.
#[component]
pub fn PreviewSurface(
    #[prop(into)] thread_id: Signal<String>,
    #[prop(into)] target_file: Signal<Option<String>>,
) -> impl IntoView {
    let candidates: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let chosen = RwSignal::new(String::new());
    let loaded: RwSignal<Option<WorkspaceFileContent>> = RwSignal::new(None);
    let loading = RwSignal::new(false);

    Effect::new(move |_| {
        if let Some(p) = target_file.get()
            && !p.is_empty()
        {
            chosen.set(p);
        }
    });

    Effect::new(move |_| {
        let tid = thread_id.get();
        if tid.is_empty() {
            candidates.set(Vec::new());
            return;
        }
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        spawn_local(async move {
            if let Ok(res) = api::request::<FilesListResp>(
                &format!("{AGENT_BASE}/threads/{tid}/files"),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
                && !signal.aborted()
            {
                let previewable: Vec<String> = res
                    .items
                    .into_iter()
                    .filter(|p| {
                        let low = p.to_ascii_lowercase();
                        low.ends_with(".html")
                            || low.ends_with(".htm")
                            || low.ends_with(".md")
                            || low.ends_with(".svg")
                    })
                    .collect();
                if chosen.with_untracked(String::is_empty)
                    && let Some(first) = previewable.first()
                {
                    chosen.try_set(first.clone());
                }
                candidates.try_set(previewable);
            }
        });
    });

    Effect::new(move |_| {
        let tid = thread_id.get();
        let path = chosen.get();
        if tid.is_empty() || path.is_empty() {
            loaded.set(None);
            return;
        }
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        loading.set(true);
        spawn_local(async move {
            if let Ok(file) = api::request::<WorkspaceFileContent>(
                &format!("{AGENT_BASE}/threads/{tid}/file?path={}", encode_uri(&path)),
                Req {
                    signal: Some(signal.clone()),
                    ..Default::default()
                },
            )
            .await
                && !signal.aborted()
            {
                loaded.try_set(Some(file));
            }
            if !signal.aborted() {
                loading.try_set(false);
            }
        });
    });

    view! {
        <div class="agent-preview-surface">
            <div class="agent-preview-bar">
                <select aria-label="Preview file" prop:value=move || chosen.get() on:change=move |e| chosen.set(event_target_value(&e))>
                    <option value="">"Choose an HTML, SVG, or Markdown file\u{2026}"</option>
                    {move || candidates.get().into_iter().map(|p| {
                        let p_lbl = p.clone();
                        view! { <option value=p>{p_lbl}</option> }
                    }).collect_view()}
                </select>
            </div>
            {move || {
                if loading.get() {
                    return view! { <p class="agent-review-note" role="status">"Loading preview\u{2026}"</p> }.into_any();
                }
                let Some(file) = loaded.get() else {
                    return view! {
                        <div class="agent-review-empty">
                            <Icon name="globe"/>
                            <h3>"Workspace preview"</h3>
                            <p>"Preview HTML, SVG, or Markdown files from this thread\u{2019}s working copy inside a strict sandbox."</p>
                        </div>
                    }.into_any();
                };
                if file.media_type == "text/markdown" || file.path.ends_with(".md") {
                    view! { <div class="agent-file-markdown"><Markdown source=file.content/></div> }.into_any()
                } else {
                    view! {
                        <iframe class="agent-preview-frame" sandbox="" title=format!("Preview {}", file.path) srcdoc=file.content/>
                    }.into_any()
                }
            }}
        </div>
    }
}

/// Project Scripts card inside the Floating Details Card.
#[component]
pub fn ProjectScriptsCard(
    #[prop(into)] project_id: Signal<String>,
    #[prop(into)] scripts: Signal<Vec<ProjectScript>>,
    on_save_scripts: Callback<Vec<ProjectScript>>,
    on_run_script: Callback<ProjectScript>,
) -> impl IntoView {
    let adding = RwSignal::new(false);
    let name = RwSignal::new(String::new());
    let command = RwSignal::new(String::new());

    let submit = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        let n = name.get_untracked().trim().to_owned();
        let c = command.get_untracked().trim().to_owned();
        if n.is_empty() || c.is_empty() {
            return;
        }
        let mut next = scripts.get_untracked();
        if next.len() >= 8 {
            return;
        }
        next.push(ProjectScript {
            id: random_uuid(),
            name: n,
            command: c,
        });
        on_save_scripts.try_run(next);
        name.set(String::new());
        command.set(String::new());
        adding.set(false);
    };

    view! {
        {move || (!project_id.with(String::is_empty)).then(|| {
            let list = scripts.get();
            view! {
                <div class="agent-scripts-section">
                    <div class="agent-scripts-head">
                        <span><Icon name="play"/>"Project scripts"</span>
                        <button type="button" class="text-button" on:click=move |_| adding.update(|v| *v = !*v)>
                            {move || if adding.get() { "Cancel" } else { "+ Add" }}
                        </button>
                    </div>
                    {(!list.is_empty()).then(|| view! {
                        <ul class="agent-scripts-list">
                            {list.into_iter().map(|s| {
                                let s_run = s.clone();
                                let id_del = s.id.clone();
                                view! {
                                    <li>
                                        <button type="button" class="agent-script-run" title=format!("Run `{}` in terminal", s.command) on:click=move |_| { on_run_script.try_run(s_run.clone()); }>
                                            <Icon name="play"/>
                                            <span>{s.name}</span>
                                        </button>
                                        <button type="button" class="icon-button" aria-label="Remove script" on:click=move |_| {
                                            let mut next = scripts.get_untracked();
                                            next.retain(|item| item.id != id_del);
                                            on_save_scripts.try_run(next);
                                        }><Icon name="close"/></button>
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    })}
                    {move || adding.get().then(|| view! {
                        <form class="agent-script-form" on:submit=submit>
                            <input type="text" placeholder="Name (e.g. Test)" maxlength="60" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/>
                            <input type="text" placeholder="Command (e.g. cargo test)" maxlength="300" prop:value=move || command.get() on:input=move |e| command.set(event_target_value(&e))/>
                            <button type="submit" class="primary-button" disabled=move || name.with(|s| s.trim().is_empty()) || command.with(|s| s.trim().is_empty())>"Save script"</button>
                        </form>
                    })}
                </div>
            }
        })}
    }
}

/// Quick helper to serialize project scripts map back into a preferences patch.
pub fn scripts_patch(
    mut map: serde_json::Map<String, Value>,
    project_id: &str,
    scripts: Vec<ProjectScript>,
) -> serde_json::Map<String, Value> {
    map.insert(project_id.to_owned(), json!(scripts));
    let mut patch = serde_json::Map::new();
    patch.insert("agentProjectScripts".into(), Value::Object(map));
    patch
}
