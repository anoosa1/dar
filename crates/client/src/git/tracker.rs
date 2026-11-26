//! Issues and pull requests (`git/tracker.tsx`): `IssuesTab`, `IssueSheet` (drawer for issues
//! and full-stage review for pull requests), `IssueEditor`, and `LabelsEditor`.

use std::collections::HashSet;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use wasm_bindgen::JsCast;

use super::diff::DiffMode;
use super::parts::{
    BranchPair, Failure, FileChanges, GIT_LABEL_COLORS, GitBranches, GitCompare, GitIssue,
    GitIssueList, GitIssueSummary, GitLabel, GitMarkdown, GitMergeResult, GitPullAgent,
    GitPullChanges, GitRepository, GitSegmented, GitSelect, GitTimelineItem, Loading, MergeBox,
    Person, Sha, SheetHead, TRACKER_LIMITS_BODY, TRACKER_LIMITS_LABEL_DESCRIPTION,
    TRACKER_LIMITS_LABEL_NAME, TRACKER_LIMITS_LABELS_PER_ITEM, TRACKER_LIMITS_PAGE,
    TRACKER_LIMITS_TITLE, encode_uri, focus_aside, focus_sheet, plural, query_string, repo_api,
    sheet_keydown, use_load,
};
use crate::api::{self, BrowserSession};
use crate::dates::{edited, local_date_time};
use crate::history::ChangeRow;
use crate::icons::Icon;
use crate::markdown::{MarkdownToolbar, format_markdown};
use crate::ui::{ActionMenu, Dialog, EditorPane, MenuAction, document, unique_id};

#[derive(Clone, Debug)]
pub enum IssueEditing {
    Issue {
        kind: String,
        issue: Option<GitIssue>,
        head: Option<String>,
        base: Option<String>,
    },
    Labels,
}

fn noun(kind: &str) -> &'static str {
    if kind == "pull" {
        "pull request"
    } else {
        "issue"
    }
}

fn writer(repo: &GitRepository) -> bool {
    repo.role != "read"
}

/// Database refusals in words (`trackerFailure`).
pub fn tracker_failure(message: &str, kind: &str) -> String {
    if message.contains("alreadyExists") {
        return if kind == "label" {
            "A label with this name already exists."
        } else {
            "A pull request between these branches is already open."
        }
        .to_owned();
    }
    if message.contains("conflict") {
        return if kind == "pull" {
            "It changed meanwhile, or its branch is gone. Reload and try again."
        } else {
            "It changed since you opened it. Reload and try again."
        }
        .to_owned();
    }
    if message.contains("forbidden") {
        return "You can read this repository but not change this. Ask its owner for write access."
            .to_owned();
    }
    if message.contains("notFound") {
        return if kind == "pull" {
            "That branch no longer exists."
        } else {
            "It no longer exists."
        }
        .to_owned();
    }
    if message.contains("operationTooLarge") {
        return "This repository has reached its limit.".to_owned();
    }
    if message.contains("invalidInput") {
        return "Check the fields and try again.".to_owned();
    }
    message.to_owned()
}

fn state_icon_name(kind: &str, state: &str) -> &'static str {
    if kind == "pull" {
        match state {
            "closed" => "pullClosed",
            "merged" => "merge",
            _ => "pull",
        }
    } else if state == "open" {
        "issue"
    } else {
        "issueDone"
    }
}

fn state_icon_class(state: &str) -> &'static str {
    match state {
        "open" => "git-state-icon is-open",
        "closed" => "git-state-icon is-closed",
        "merged" => "git-state-icon is-merged",
        _ => "git-state-icon",
    }
}

#[component]
fn StateIcon(#[prop(into)] kind: String, #[prop(into)] state: String) -> impl IntoView {
    let icon = state_icon_name(&kind, &state);
    let class = state_icon_class(&state);
    view! { <Icon name=icon class=class/> }
}

#[component]
fn StateChip(#[prop(into)] kind: String, #[prop(into)] state: String) -> impl IntoView {
    let label = match state.as_str() {
        "open" => "Open",
        "merged" => "Merged",
        _ => "Closed",
    };
    let class = format!("git-state is-{state}");
    view! { <span class=class><StateIcon kind=kind state=state/>{label}</span> }
}

#[component]
fn LabelChip(label: GitLabel) -> impl IntoView {
    let class = format!("git-label is-{}", label.color);
    let title = (!label.description.is_empty()).then_some(label.description);
    view! { <span class=class title=title>{label.name}</span> }
}

#[component]
fn Labels(ids: Vec<String>, labels: Vec<GitLabel>) -> impl IntoView {
    let known: Vec<GitLabel> = ids
        .iter()
        .filter_map(|id| labels.iter().find(|l| l.id == *id).cloned())
        .collect();
    (!known.is_empty()).then(|| {
        view! {
            <span class="git-labels">
                {known.into_iter().map(|label| view! { <LabelChip label=label/> }).collect_view()}
            </span>
        }
    })
}

// ---- Lists --------------------------------------------------------------------------------------

#[component]
pub fn IssuesTab(
    repo: GitRepository,
    kind: &'static str,
    #[prop(into)] version: Signal<u64>,
    #[prop(into)] selected: Signal<Option<u32>>,
    on_open: Callback<u32>,
    on_edit: Callback<IssueEditing>,
) -> impl IntoView {
    let state = RwSignal::new("open".to_owned());
    let label = RwSignal::new(String::new());
    let query = RwSignal::new(String::new());
    let repo_id = StoredValue::new(repo.id.clone());
    let can_write = writer(&repo);
    let can_create = kind == "issue" || can_write;
    let title = if kind == "pull" {
        "Pull requests"
    } else {
        "Issues"
    };
    let item_noun = noun(kind);

    let url = Signal::derive(move || {
        let st = state.get();
        let lbl = label.get();
        let mut pairs = vec![("kind", kind), ("state", st.as_str())];
        if !lbl.is_empty() {
            pairs.push(("label", lbl.as_str()));
        }
        Some(format!(
            "{}/issues?{}",
            repo_api(&repo_id.get_value()),
            query_string(&pairs)
        ))
    });
    let loaded = use_load::<GitIssueList>(url, version);

    let seg_options = Signal::derive(move || {
        let (open_str, closed_str) = loaded.value.with(|l| match l {
            Some(list) => (
                format!("Open {}", list.open),
                format!("Closed {}", list.closed),
            ),
            None => ("Open".to_owned(), "Closed".to_owned()),
        });
        vec![
            ("open".to_owned(), open_str),
            ("closed".to_owned(), closed_str),
        ]
    });

    let label_options = Signal::derive(move || {
        let mut opts = vec![(String::new(), "All labels".to_owned())];
        loaded.value.with(|l| {
            if let Some(list) = l {
                for item in &list.labels {
                    opts.push((item.id.clone(), item.name.clone()));
                }
            }
        });
        opts
    });

    let search_placeholder = if kind == "pull" {
        "Filter pull requests\u{2026}"
    } else {
        "Filter issues\u{2026}"
    };
    let search_label = if kind == "pull" {
        "Filter pull requests"
    } else {
        "Filter issues"
    };

    view! {
        <div class="git-scroll git-issues">
            <div class="git-toolbar">
                <GitSegmented
                    label=format!("{title} to show")
                    value=state
                    options=seg_options
                    on_change=Callback::new(move |v| state.set(v))
                />
                {move || {
                    let has_labels = loaded.value.with(|l| l.as_ref().is_some_and(|v| !v.labels.is_empty()));
                    has_labels.then(|| view! {
                        <GitSelect
                            aria_label="Label"
                            value=label
                            options=label_options
                            on_change=Callback::new(move |v| label.set(v))
                        />
                    })
                }}
                <label class="git-inline-input">
                    <Icon name="search"/>
                    <input
                        type="search"
                        placeholder=search_placeholder
                        aria-label=search_label
                        prop:value=move || query.get()
                        on:input=move |e| query.set(event_target_value(&e))
                    />
                </label>
                <span class="git-toolbar-end">
                    {can_write.then(|| view! {
                        <button type="button" class="secondary-button git-tool-button" on:click=move |_| { let _ = on_edit.try_run(IssueEditing::Labels); }>
                            <Icon name="label"/>
                            "Labels"
                        </button>
                    })}
                    {can_create.then(|| view! {
                        <button
                            type="button"
                            class="primary-button git-tool-button"
                            on:click=move |_| {
                                let _ = on_edit.try_run(IssueEditing::Issue {
                                    kind: kind.to_owned(),
                                    issue: None,
                                    head: None,
                                    base: None,
                                });
                            }
                        >
                            <Icon name="plus"/>
                            {format!("New {item_noun}")}
                        </button>
                    })}
                </span>
            </div>
            {move || {
                if !loaded.error.with(String::is_empty) {
                    return view! { <Failure error=loaded.error retry=loaded.retry what=title/> }.into_any();
                }
                let Some(list) = loaded.value.get() else {
                    return view! { <Loading label=format!("Loading {}", title.to_lowercase())/> }.into_any();
                };
                if list.items.is_empty() {
                    let st = state.get();
                    let has_label = !label.with(String::is_empty);
                    let empty_heading = if has_label {
                        format!("No {st} {item_noun}s with this label")
                    } else {
                        format!("No {st} {item_noun}s")
                    };
                    let empty_body = if kind == "pull" {
                        if st == "open" {
                            "Open a pull request to discuss a branch before it is merged. Merging it, here or with Git, closes it."
                        } else {
                            "Merged and closed pull requests appear here."
                        }
                    } else if st == "open" {
                        "Track bugs, ideas and tasks for this repository. Anyone it is shared with can open issues and comment."
                    } else {
                        "Closed issues appear here."
                    };
                    return view! {
                        <div class="git-empty git-empty-inline">
                            <span class="empty-icon"><Icon name=if kind == "pull" { "pull" } else { "issue" }/></span>
                            <h2>{empty_heading}</h2>
                            <p>{empty_body}</p>
                        </div>
                    }.into_any();
                }
                let needle = query.get().trim().to_ascii_lowercase();
                let filtered: Vec<GitIssueSummary> = if needle.is_empty() {
                    list.items
                } else {
                    list.items
                        .into_iter()
                        .filter(|item| {
                            item.title.to_ascii_lowercase().contains(&needle)
                                || item.author.to_ascii_lowercase().contains(&needle)
                                || item.number.to_string() == needle.trim_start_matches('#')
                        })
                        .collect()
                };
                if filtered.is_empty() {
                    return view! {
                        <p class="git-hint">
                            {format!("No {item_noun}s match \u{201c}{}\u{201d}. ", query.get())}
                            <button type="button" class="git-link" on:click=move |_| query.set(String::new())>"Clear filter"</button>
                        </p>
                    }.into_any();
                }
                let all_labels = list.labels.clone();
                view! {
                    <ul class="git-issue-list" aria-label=title>
                        {filtered.into_iter().map(|item| {
                            let num = item.number;
                            let labels = all_labels.clone();
                            let is_sel = Signal::derive(move || selected.get() == Some(num));
                            view! {
                                <li>
                                    <IssueRow
                                        item=item
                                        labels=labels
                                        selected=is_sel
                                        on_open=Callback::new(move |_| { let _ = on_open.try_run(num); })
                                    />
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                }.into_any()
            }}
            {move || loaded.value.with(|l| l.as_ref().is_some_and(|v| v.truncated)).then(|| view! {
                <p class="git-hint">{format!("Only the newest {TRACKER_LIMITS_PAGE} are listed.")}</p>
            })}
        </div>
    }
}

#[component]
fn IssueRow(
    item: GitIssueSummary,
    labels: Vec<GitLabel>,
    #[prop(into)] selected: Signal<bool>,
    on_open: Callback<()>,
) -> impl IntoView {
    let time_text = if item.state == "open" {
        format!("opened {}", edited(item.created_at))
    } else {
        let verb = if item.state == "merged" {
            "merged"
        } else {
            "closed"
        };
        format!(
            "{verb} {}",
            edited(item.closed_at.unwrap_or(item.updated_at))
        )
    };
    let pair = if item.kind == "pull" {
        item.head.clone().zip(item.base.clone())
    } else {
        None
    };
    let is_agent = item.agent.unwrap_or(false);
    let comments = item.comments;

    view! {
        <button
            type="button"
            class=move || if selected.get() { "git-issue-row is-selected" } else { "git-issue-row" }
            aria-pressed=move || selected.get().to_string()
            on:click=move |_| { let _ = on_open.try_run(()); }
        >
            <StateIcon kind=item.kind.clone() state=item.state.clone()/>
            <span class="git-issue-main">
                <span class="git-issue-title">
                    <span>{item.title}</span>
                    <Labels ids=item.labels labels=labels/>
                </span>
                <span class="git-issue-sub">
                    {format!("#{} \u{00b7} {time_text} by {}", item.number, item.author)}
                    {pair.map(|(head, base)| view! { " \u{00b7} "<BranchPair head=head base=base/> })}
                    {is_agent.then(|| view! { <span class="git-source is-agent"><Icon name="agent"/>"Agent"</span> })}
                </span>
            </span>
            <span class="git-issue-end">
                {(comments > 0).then(|| view! {
                    <span class="git-muted" title=plural(comments, "comment", None)>
                        <Icon name="comment"/>
                        {comments}
                        <span class="sr-only">" comments"</span>
                    </span>
                })}
            </span>
        </button>
    }
}

// ---- One issue or pull request ------------------------------------------------------------------

#[component]
pub fn IssueSheet(
    repo: GitRepository,
    number: u32,
    session: BrowserSession,
    #[prop(into)] version: Signal<u64>,
    diff_mode_str: RwSignal<String>,
    wrap: RwSignal<bool>,
    #[prop(optional)] full_stage: bool,
    expanded: Signal<bool>,
    on_expand: Callback<()>,
    #[prop(optional)] on_kind_known: Option<Callback<bool>>,
    notify: Callback<String>,
    on_close: Callback<()>,
    on_open: Callback<u32>,
    on_commit: Callback<String>,
    on_edit: Callback<IssueEditing>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let drawer_root = NodeRef::<html::Div>::new();
    let stage_root = NodeRef::<html::Aside>::new();
    if full_stage {
        focus_aside(stage_root);
    } else {
        focus_sheet(drawer_root);
    }

    let repo_sv = StoredValue::new(repo.clone());
    let session_sv = StoredValue::new(session);
    let url = StoredValue::new(format!("{}/issues/{number}", repo_api(&repo.id)));
    let repo_rev = repo.revision;

    let issue_trigger = Signal::derive(move || version.get().wrapping_add(repo_rev));
    let issue_url = Signal::derive(move || Some(url.get_value()));
    let issue_load = use_load::<GitIssue>(issue_url, issue_trigger);

    if let Some(cb) = on_kind_known {
        Effect::new(move |_| {
            if let Some(is_pull) = issue_load
                .value
                .with(|i| i.as_ref().map(|v| v.kind == "pull"))
                && is_pull != full_stage
            {
                let _ = cb.try_run(is_pull);
            }
        });
    }

    let list_url = Signal::derive(move || {
        Some(format!(
            "{}/issues?kind=issue&state=open",
            repo_api(&repo_sv.with_value(|r| r.id.clone()))
        ))
    });
    let list_load = use_load::<GitIssueList>(list_url, version);

    let changes_url = Signal::derive(move || {
        issue_load.value.with(|i| {
            i.as_ref()
                .filter(|v| v.kind == "pull")
                .map(|_| format!("{}/changes", url.get_value()))
        })
    });
    let changes_trigger = Signal::derive(move || {
        let state_tag = issue_load
            .value
            .with(|i| match i.as_ref().map(|v| v.state.as_str()) {
                Some("open") => 1u64,
                Some("closed") => 2,
                Some("merged") => 3,
                _ => 0,
            });
        version
            .get()
            .wrapping_mul(31)
            .wrapping_add(repo_rev)
            .wrapping_add(state_tag)
    });
    let changes_load = use_load::<GitPullChanges>(changes_url, changes_trigger);

    let view_mode = RwSignal::new("conversation".to_owned());
    let diff_mode = Signal::derive(move || {
        if diff_mode_str.get() == "split" {
            DiffMode::Split
        } else {
            DiffMode::Unified
        }
    });
    let diff_options = Signal::derive(move || {
        vec![
            ("unified".to_owned(), "Unified".to_owned()),
            ("split".to_owned(), "Split".to_owned()),
        ]
    });
    let viewed: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let open_files: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let initialized_open = RwSignal::new(false);
    let selected_file: RwSignal<Option<String>> = RwSignal::new(None);
    let rail_open = RwSignal::new(true);

    Effect::new(move |_| {
        if initialized_open.get_untracked() {
            return;
        }
        if let Some(paths) = changes_load.value.with(|c| {
            c.as_ref()
                .and_then(|v| v.compare.as_ref())
                .map(|cmp| cmp.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>())
        }) && !paths.is_empty()
        {
            selected_file.set(paths.first().cloned());
            open_files.set(paths.into_iter().collect());
            initialized_open.set(true);
        }
    });

    let busy = RwSignal::new(false);
    let failure = RwSignal::new(String::new());
    // What the comment composer shows; it is rebuilt only when these change.
    let composer_key = Memo::new(move |_| {
        issue_load.value.with(|i| {
            i.as_ref().map(|i| {
                (
                    i.number,
                    i.kind.clone(),
                    i.state.clone(),
                    i.permissions.close,
                )
            })
        })
    });
    let failed = Memo::new(move |_| !issue_load.error.with(String::is_empty));

    let reload_issue = Callback::new(move |_| {
        let u = url.get_value();
        spawn_local(async move {
            match api::get::<GitIssue>(&u).await {
                Ok(next) => {
                    issue_load.set(next);
                    let _ = on_changed.try_run(());
                }
                Err(cause) => {
                    let k = issue_load.value.with_untracked(|i| {
                        i.as_ref()
                            .map(|v| v.kind.clone())
                            .unwrap_or_else(|| "issue".to_owned())
                    });
                    let _ = failure.try_set(tracker_failure(&cause.message, &k));
                }
            }
        });
    });

    let change_issue = Callback::new(
        move |(mut update, done): (Map<String, Value>, Option<String>)| {
            let Some(current) = issue_load.value.get_untracked() else {
                return;
            };
            busy.set(true);
            failure.set(String::new());
            update.insert("revision".into(), current.revision.into());
            let u = url.get_value();
            let token = session_sv.with_value(|s| s.csrf_token.clone());
            let kind = current.kind.clone();
            spawn_local(async move {
                match api::send::<Value>(&u, "PATCH", Some(Value::Object(update)), &token).await {
                    Ok(_) => match api::get::<GitIssue>(&u).await {
                        Ok(next) => {
                            issue_load.set(next);
                            if let Some(msg) = done {
                                notify.run(msg);
                            }
                            let _ = on_changed.try_run(());
                        }
                        Err(cause) => {
                            let _ = failure.try_set(tracker_failure(&cause.message, &kind));
                        }
                    },
                    Err(cause) => {
                        let _ = failure.try_set(tracker_failure(&cause.message, &kind));
                    }
                }
                let _ = busy.try_set(false);
            });
        },
    );

    let title_view = view! {
        {move || {
            let pull = issue_load.value.with(|i| i.as_ref().is_some_and(|v| v.kind == "pull"));
            format!("{} #{number}", if pull { "Pull request" } else { "Issue" })
        }}
    }
    .into_any();

    let pull_view_options = Signal::derive(move || {
        let (commits_lbl, files_lbl) = changes_load.value.with(|ch| match ch {
            Some(c) => {
                let commits = match &c.compare {
                    Some(cmp) => format!("Commits {}", cmp.ahead.len()),
                    None => "Commits".to_owned(),
                };
                let files = if let Some(cmp) = &c.compare {
                    format!("Files {}", cmp.files.len())
                } else if let Some(ag) = &c.agent {
                    format!("Files {}", ag.changes.len())
                } else {
                    "Files".to_owned()
                };
                (commits, files)
            }
            None => ("Commits".to_owned(), "Files".to_owned()),
        });
        vec![
            ("conversation".to_owned(), "Conversation".to_owned()),
            ("commits".to_owned(), commits_lbl),
            ("files".to_owned(), files_lbl),
        ]
    });

    let is_pull_memo = Memo::new(move |_| {
        issue_load
            .value
            .with(|i| i.as_ref().is_some_and(|v| v.kind == "pull"))
    });
    let show_conv = Memo::new(move |_| {
        !failed.get() && (!is_pull_memo.get() || view_mode.get() == "conversation")
    });

    let inner_body = view! {
        {move || {
            let kind_str = issue_load.value.with(|i| i.as_ref().map(|v| v.kind.clone()).unwrap_or_else(|| "issue".to_owned()));
            if !issue_load.error.with(String::is_empty) {
                let msg = Signal::derive(move || tracker_failure(&issue_load.error.get(), &kind_str));
                let what = format!("This {}", noun(&issue_load.value.with_untracked(|i| i.as_ref().map(|v| v.kind.clone()).unwrap_or_else(|| "issue".to_owned()))));
                return view! { <div class="git-sheet-pad"><Failure error=msg retry=issue_load.retry what=what/></div> }.into_any();
            }
            let Some(issue) = issue_load.value.get() else {
                return view! { <div class="git-sheet-pad"><Loading label=format!("Loading {}", noun(&kind_str))/></div> }.into_any();
            };
            let labels = list_load.value.with(|l| l.as_ref().map(|v| v.labels.clone()).unwrap_or_default());
            let pull = issue.kind == "pull";
            let issue_sv = StoredValue::new(issue.clone());
            let labels_sv = StoredValue::new(labels.clone());
            let pair = issue.head.clone().zip(issue.base.clone());
            let is_agent = issue.agent.unwrap_or(false);

            let label_actions = Callback::new(move |_: ()| {
                let iss = issue_sv.get_value();
                let all = labels_sv.get_value();
                let is_busy = busy.get_untracked();
                let mut acts: Vec<MenuAction> = all
                    .iter()
                    .map(|item| {
                        let has = iss.labels.contains(&item.id);
                        let disabled = is_busy || (!has && iss.labels.len() >= TRACKER_LIMITS_LABELS_PER_ITEM);
                        let next_labels: Vec<String> = if has {
                            iss.labels.iter().filter(|id| *id != &item.id).cloned().collect()
                        } else {
                            let mut v = iss.labels.clone();
                            v.push(item.id.clone());
                            v
                        };
                        MenuAction::new(item.name.clone(), "label")
                            .current(has)
                            .disabled(disabled)
                            .run(move || {
                                let mut map = Map::new();
                                map.insert("labels".into(), json!(next_labels));
                                change_issue.run((map, None));
                            })
                    })
                    .collect();
                let mut manage = MenuAction::new("Manage labels", "settings")
                    .run(move || { let _ = on_edit.try_run(IssueEditing::Labels); });
                if !all.is_empty() {
                    manage = manage.separator();
                }
                acts.push(manage);
                acts
            });

            view! {
                <div class=if full_stage { "git-pr-top" } else { "git-issue-head" }>
                    <div class="git-pr-top-main">
                        <h3 class="git-commit-title">{issue.title.clone()}</h3>
                        <div class="git-issue-meta">
                            <StateChip kind=issue.kind.clone() state=issue.state.clone()/>
                            <span><strong>{issue.author.clone()}</strong>{format!(" opened {}", edited(issue.created_at))}</span>
                            {pair.map(|(h, b)| view! { <BranchPair head=h base=b/> })}
                            {is_agent.then(|| view! { <span class="git-source is-agent"><Icon name="agent"/>"Agent"</span> })}
                        </div>
                        {(!issue.labels.is_empty() || issue.permissions.label).then(|| view! {
                            <div class="git-issue-labels">
                                <Labels ids=issue.labels.clone() labels=labels.clone()/>
                                {issue.labels.is_empty().then(|| view! { <span class="git-muted">"No labels"</span> })}
                                {issue.permissions.label.then(|| view! {
                                    <ActionMenu label="Labels" class="git-label-menu" align_start=true actions=label_actions>
                                        <Icon name="label"/>
                                        <span>"Edit labels"</span>
                                    </ActionMenu>
                                })}
                            </div>
                        })}
                    </div>
                </div>
                {pull.then(|| view! {
                    <div class=if full_stage { "git-pr-subbar" } else { "git-issue-tabs" }>
                        <GitSegmented
                            label="Pull request view"
                            value=view_mode
                            options=pull_view_options
                            on_change=Callback::new(move |v| view_mode.set(v))
                        />
                    </div>
                })}
            }.into_any()
        }}
        {move || (!failure.with(String::is_empty)).then(|| view! { <p class="error git-sheet-pad" role="alert">{failure.get()}</p> })}
        {move || {
            show_conv.get().then(|| view! {
                <div class=if full_stage { "git-pr-conv-scroll" } else { "" }>
                    <div class=if full_stage { "git-conv-col" } else { "git-drawer-conv" }>
                        {move || {
                            let Some(iss) = issue_load.value.get() else {
                                return ().into_any();
                            };
                            let pull = iss.kind == "pull";
                            let show_merge = pull && iss.state == "open" && iss.permissions.merge;
                            let iss_m = iss.clone();
                            view! {
                                <Timeline
                                    issue=iss
                                    repo=repo_sv.get_value()
                                    session=session_sv.get_value()
                                    on_open=on_open
                                    on_changed=reload_issue
                                />
                                {show_merge.then(|| move || {
                                    if !changes_load.error.with(String::is_empty) {
                                        let err = Signal::derive(move || tracker_failure(&changes_load.error.get(), "pull"));
                                        return view! { <Failure error=err retry=changes_load.retry what="The branches"/> }.into_any();
                                    }
                                    let Some(ch) = changes_load.value.get() else {
                                        return view! { <Loading label="Checking the branches"/> }.into_any();
                                    };
                                    if let Some(note) = ch.note {
                                        return view! {
                                            <div class="git-banner">
                                                <Icon name="alert"/>
                                                <div>
                                                    <strong>"This pull request can\u{2019}t be merged"</strong>
                                                    <span>{note}</span>
                                                </div>
                                            </div>
                                        }.into_any();
                                    }
                                    if let Some(cmp) = ch.compare {
                                        if !cmp.ahead.is_empty() {
                                            let default_msg = format!(
                                                "Merge pull request #{} from {}\n\n{}",
                                                iss_m.number,
                                                iss_m.head.as_deref().unwrap_or(""),
                                                iss_m.title
                                            );
                                            let base_n = iss_m.base.clone().unwrap_or_default();
                                            let head_n = iss_m.head.clone().unwrap_or_default();
                                            let num = iss_m.number;
                                            return view! {
                                                <MergeBox
                                                    repo=repo_sv.get_value()
                                                    compare=cmp
                                                    session=session_sv.get_value()
                                                    default_message=default_msg
                                                    on_merged=Callback::new(move |result: GitMergeResult| {
                                                        let msg = if result.fast_forward {
                                                            format!("{base_n} now points at {head_n}")
                                                        } else {
                                                            format!("Merged #{num}")
                                                        };
                                                        notify.run(msg);
                                                        reload_issue.run(());
                                                    })
                                                />
                                            }.into_any();
                                        } else {
                                            let base_n = iss_m.base.clone().unwrap_or_default();
                                            let head_n = iss_m.head.clone().unwrap_or_default();
                                            return view! {
                                                <div class="git-banner is-info">
                                                    <Icon name="info"/>
                                                    <div>
                                                        <strong>"Nothing to merge"</strong>
                                                        <span>{format!("{base_n} already has every commit on {head_n}. Push commits to the branch, or close this pull request.")}</span>
                                                    </div>
                                                </div>
                                            }.into_any();
                                        }
                                    }
                                    if let Some(agent) = ch.agent {
                                        let num = iss_m.number;
                                        return view! {
                                            <AgentMerge
                                                repo=repo_sv.get_value()
                                                issue=iss_m.clone()
                                                agent=agent
                                                session=session_sv.get_value()
                                                on_merged=Callback::new(move |_| {
                                                    notify.run(format!("Merged #{num} into the folder"));
                                                    reload_issue.run(());
                                                })
                                            />
                                        }.into_any();
                                    }
                                    ().into_any()
                                })}
                            }.into_any()
                        }}
                        // Own reactive region keyed strictly on `(number, kind, state, permissions.close)`
                        // so label toggles and timeline refreshes never wipe typed drafts.
                        {move || {
                            let (num, _, _, _) = composer_key.get()?;
                            if failed.get() {
                                return None;
                            }
                            let issue = issue_load.value.get_untracked()?;
                            Some(view! {
                                <Composer
                                    issue=issue
                                    repo=repo_sv.get_value()
                                    session=session_sv.get_value()
                                    busy=busy
                                    on_open=on_open
                                    on_close=Callback::new(move |st: &'static str| {
                                        let mut map = Map::new();
                                        map.insert("state".into(), st.into());
                                        let msg = if st == "closed" { format!("Closed #{num}") } else { format!("Reopened #{num}") };
                                        change_issue.run((map, Some(msg)));
                                    })
                                    on_commented=reload_issue
                                />
                            })
                        }}
                    </div>
                </div>
            })
        }}
        {move || {
            let current_view = view_mode.get();
            if failed.get() || !is_pull_memo.get() || current_view == "conversation" {
                return ().into_any();
            }
            if !changes_load.error.with(String::is_empty) {
                let err = Signal::derive(move || tracker_failure(&changes_load.error.get(), "pull"));
                return view! { <div class="git-sheet-pad"><Failure error=err retry=changes_load.retry what="The changes"/></div> }.into_any();
            }
            let Some(ch) = changes_load.value.get() else {
                return view! { <div class="git-sheet-pad"><Loading label="Loading changes"/></div> }.into_any();
            };
            if let Some(note) = &ch.note
                && ch.compare.is_none()
            {
                return view! { <p class="git-hint git-sheet-pad">{note.clone()}</p> }.into_any();
            }
            if current_view == "commits" {
                if let Some(cmp) = ch.compare {
                    if !cmp.ahead.is_empty() {
                        view! {
                            <div class=if full_stage { "git-pr-conv-scroll" } else { "" }>
                                <div class=if full_stage { "git-conv-col" } else { "" }>
                                    <ol class="git-mini-commits">
                                        {cmp.ahead.into_iter().map(|commit| {
                                            let sha = commit.sha.clone();
                                            let sha_click = sha.clone();
                                            view! {
                                                <li>
                                                    <button type="button" on:click=move |_| { let _ = on_commit.try_run(sha_click.clone()); }>
                                                        <Person name=commit.author source=commit.source/>
                                                        <span>{commit.subject}</span>
                                                        <Sha sha=sha/>
                                                        <small>{edited(commit.time)}</small>
                                                    </button>
                                                </li>
                                            }
                                        }).collect_view()}
                                    </ol>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        let (head_name, base_name) = issue_load.value.with_untracked(|i| {
                            i.as_ref()
                                .map(|v| (v.head.clone().unwrap_or_default(), v.base.clone().unwrap_or_default()))
                                .unwrap_or_default()
                        });
                        view! {
                            <p class="git-hint git-sheet-pad">{format!(
                                "{head_name} has no commits that {base_name} doesn\u{2019}t."
                            )}</p>
                        }.into_any()
                    }
                } else {
                    view! {
                        <p class="git-hint git-sheet-pad">
                            "Agent changes are not commits yet: merging them commits them to main. "
                            {ch.agent.map(|ag| view! {
                                <a href=format!("/agent?thread={}", encode_uri(&ag.thread_id))>"Open the thread in Agent"</a>
                            })}
                        </p>
                    }.into_any()
                }
            } else if let Some(cmp) = ch.compare {
                let added_total: usize = cmp.files.iter().map(|f| f.added).sum();
                let removed_total: usize = cmp.files.iter().map(|f| f.removed).sum();
                let file_count = cmp.files.len();
                let all_paths: Vec<String> = cmp.files.iter().map(|f| f.path.clone()).collect();
                let all_paths_toggle = all_paths.clone();
                let files_list = cmp.files.clone();
                view! {
                    <div class=move || {
                        if full_stage && rail_open.get() {
                            "git-pr-files-layout"
                        } else {
                            "git-pr-files-layout is-rail-hidden"
                        }
                    }>
                        {full_stage.then(|| {
                            let rail_files = files_list.clone();
                            let total_files = rail_files.len();
                            view! {
                                <div class="git-pr-files-rail" aria-label="Changed files">
                                    <div class="git-file-tree-head">
                                        <span>"Changed files"</span>
                                        <span class="git-count">{move || {
                                            let v = viewed.with(HashSet::len);
                                            format!("{v}/{total_files}")
                                        }}</span>
                                    </div>
                                    {rail_files.into_iter().enumerate().map(|(idx, file)| {
                                        let p_sel = file.path.clone();
                                        let p_act = file.path.clone();
                                        let p_chk = file.path.clone();
                                        let p_chg = file.path.clone();
                                        let p_open = file.path.clone();
                                        let short_name = file.path.rsplit('/').next().unwrap_or(&file.path).to_owned();
                                        let plus = file.added;
                                        let minus = file.removed;
                                        view! {
                                            <div class=move || {
                                                if selected_file.get().as_deref() == Some(p_act.as_str()) {
                                                    "git-changed-item is-active"
                                                } else {
                                                    "git-changed-item"
                                                }
                                            }>
                                                <input
                                                    type="checkbox"
                                                    class="git-viewed-chk"
                                                    aria-label=format!("Mark {} viewed", file.path)
                                                    prop:checked=move || viewed.with(|s| s.contains(&p_chk))
                                                    on:change=move |ev| {
                                                        let checked = event_target_checked(&ev);
                                                        let p = p_chg.clone();
                                                        viewed.update(|s| {
                                                            if checked {
                                                                s.insert(p);
                                                            } else {
                                                                s.remove(&p);
                                                            }
                                                        });
                                                    }
                                                />
                                                <button
                                                    type="button"
                                                    class="git-changed-item-btn"
                                                    title=file.path.clone()
                                                    on:click=move |_| {
                                                        selected_file.set(Some(p_sel.clone()));
                                                        open_files.update(|s| {
                                                            s.insert(p_open.clone());
                                                        });
                                                        if let Ok(list) = document().query_selector_all(".git-diff-stage .git-diff")
                                                            && let Some(node) = list.item(idx as u32)
                                                            && let Ok(el) = node.dyn_into::<web_sys::Element>()
                                                        {
                                                            el.scroll_into_view();
                                                        }
                                                    }
                                                >
                                                    <span class="git-changed-name">{short_name}</span>
                                                    <span class="git-stat">
                                                        <span class="git-added">{format!("+{plus}")}</span>
                                                        " "
                                                        <span class="git-removed">{format!("\u{2212}{minus}")}</span>
                                                    </span>
                                                </button>
                                            </div>
                                        }
                                    }).collect_view()}
                                </div>
                            }
                        })}
                        <div class="git-diff-stage">
                            <div class="git-files-summary">
                                {full_stage.then(|| view! {
                                    <button
                                        type="button"
                                        class="icon-button"
                                        aria-label="Toggle changed files sidebar"
                                        aria-pressed=move || rail_open.get().to_string()
                                        on:click=move |_| rail_open.update(|v| *v = !*v)
                                    >
                                        <Icon name="sidebar"/>
                                    </button>
                                })}
                                <strong>{format!("{} changed", plural(file_count, "file", None))}</strong>
                                <span class="git-stat">
                                    <span class="git-added">{format!("+{added_total}")}</span>
                                    " "
                                    <span class="git-removed">{format!("\u{2212}{removed_total}")}</span>
                                </span>
                                <span class="git-toolbar-end">
                                    <button
                                        type="button"
                                        class="text-button"
                                        on:click=move |_| {
                                            let all_open = open_files.with(|s| all_paths_toggle.iter().all(|p| s.contains(p)));
                                            if all_open {
                                                open_files.set(HashSet::new());
                                            } else {
                                                open_files.set(all_paths_toggle.iter().cloned().collect());
                                            }
                                        }
                                    >
                                        {move || {
                                            let all_open = open_files.with(|s| all_paths.iter().all(|p| s.contains(p)));
                                            if all_open { "Collapse all" } else { "Expand all" }
                                        }}
                                    </button>
                                    <GitSegmented
                                        label="Diff layout"
                                        value=diff_mode_str
                                        options=diff_options
                                        on_change=Callback::new(move |val| diff_mode_str.set(val))
                                    />
                                    <label class="git-wrap">
                                        <input type="checkbox" prop:checked=move || wrap.get() on:change=move |e| wrap.set(event_target_checked(&e))/>
                                        "Wrap lines"
                                    </label>
                                </span>
                            </div>
                            <FileChanges files=cmp.files wrap=wrap mode=diff_mode viewed=viewed open_files=open_files/>
                        </div>
                    </div>
                }.into_any()
            } else if let Some(ag) = ch.agent {
                let rid = repo_sv.with_value(|r| r.id.clone());
                view! {
                    <div class=if full_stage { "git-pr-conv-scroll" } else { "" }>
                        <div class="history-changes">
                            {ag.changes.into_iter().map(|item| view! {
                                <ChangeRow folder_id=rid.clone() change=item live=false/>
                            }).collect_view()}
                        </div>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }
        }}
    };

    if full_stage {
        view! {
            <aside
                node_ref=stage_root
                class="git-stage-pane git-pr-stage"
                aria-label="Conversation"
                tabindex="-1"
                on:keydown=move |e| sheet_keydown(e, on_close)
            >
                <SheetHead
                    title=title_view
                    expanded=expanded
                    on_expand=on_expand
                    on_close=on_close
                >
                    {move || issue_load.value.get().filter(|i| i.permissions.edit).map(|iss| {
                        let k = iss.kind.clone();
                        let label = format!("Edit {}", noun(&k));
                        view! {
                            <button
                                type="button"
                                class="icon-button"
                                aria-label=label
                                title="Edit"
                                on:click=move |_| {
                                    let _ = on_edit.try_run(IssueEditing::Issue {
                                        kind: k.clone(),
                                        issue: Some(iss.clone()),
                                        head: None,
                                        base: None,
                                    });
                                }
                            >
                                <Icon name="pencil"/>
                            </button>
                        }
                    })}
                </SheetHead>
                <div class="git-pr-stage-body">
                    {inner_body}
                </div>
            </aside>
        }
        .into_any()
    } else {
        view! {
            <div
                node_ref=drawer_root
                class="git-sheet"
                tabindex="-1"
                on:keydown=move |e| sheet_keydown(e, on_close)
            >
                <SheetHead
                    title=title_view
                    expanded=expanded
                    on_expand=on_expand
                    on_close=on_close
                >
                    {move || issue_load.value.get().filter(|i| i.permissions.edit).map(|iss| {
                        let k = iss.kind.clone();
                        let label = format!("Edit {}", noun(&k));
                        view! {
                            <button
                                type="button"
                                class="icon-button"
                                aria-label=label
                                title="Edit"
                                on:click=move |_| {
                                    let _ = on_edit.try_run(IssueEditing::Issue {
                                        kind: k.clone(),
                                        issue: Some(iss.clone()),
                                        head: None,
                                        base: None,
                                    });
                                }
                            >
                                <Icon name="pencil"/>
                            </button>
                        }
                    })}
                </SheetHead>
                <div class="git-sheet-body">
                    {inner_body}
                </div>
            </div>
        }
        .into_any()
    }
}

#[component]
fn Timeline(
    issue: GitIssue,
    repo: GitRepository,
    session: BrowserSession,
    on_open: Callback<u32>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let created_title = local_date_time(
        issue.created_at,
        &[("dateStyle", "medium"), ("timeStyle", "short")],
    );
    let desc_label = format!("Description by {}", issue.author);
    view! {
        <ol class="git-timeline" aria-label="Conversation">
            <li>
                <article class="git-comment" aria-label=desc_label>
                    <header>
                        <Person name=issue.author.clone()/>
                        <strong>{issue.author.clone()}</strong>
                        <span class="git-muted" title=created_title>{edited(issue.created_at)}</span>
                    </header>
                    <div class="git-comment-body">
                        {if !issue.body.is_empty() {
                            view! { <GitMarkdown source=issue.body on_reference=on_open/> }.into_any()
                        } else {
                            view! { <p class="git-muted">"No description."</p> }.into_any()
                        }}
                    </div>
                </article>
            </li>
            {issue.timeline.into_iter().map(|item| {
                let repo = repo.clone();
                let session = session.clone();
                view! {
                    <li>
                        {match item {
                            GitTimelineItem::Comment { id, author, body, created_at, edited_at, can_edit, can_delete } => view! {
                                <Comment
                                    id=id
                                    author=author
                                    body=body
                                    created_at=created_at
                                    edited_at=edited_at
                                    can_edit=can_edit
                                    can_delete=can_delete
                                    repo=repo
                                    session=session
                                    on_open=on_open
                                    on_changed=on_changed
                                />
                            }.into_any(),
                            GitTimelineItem::Event { event, author, created_at, number, title, from, to, label, sha, .. } => view! {
                                <TimelineEvent
                                    event=event
                                    author=author
                                    created_at=created_at
                                    number=number
                                    title=title
                                    from=from
                                    to=to
                                    label=label
                                    sha=sha
                                    on_open=on_open
                                />
                            }.into_any(),
                        }}
                    </li>
                }
            }).collect_view()}
        </ol>
    }
}

#[component]
fn TimelineEvent(
    event: String,
    author: String,
    created_at: f64,
    number: Option<u32>,
    title: Option<String>,
    from: Option<String>,
    to: Option<String>,
    label: Option<String>,
    sha: Option<String>,
    on_open: Callback<u32>,
) -> impl IntoView {
    let ref_btn = move |n: u32| {
        view! {
            <button type="button" class="git-link git-ref-link" on:click=move |_| { let _ = on_open.try_run(n); }>
                {format!("#{n}")}
            </button>
        }
    };
    let (icon, body): (&'static str, AnyView) = match event.as_str() {
        "closed" => (
            "issueDone",
            match number {
                Some(n) => view! { "closed this with "{ref_btn(n)} }.into_any(),
                None => "closed this".into_any(),
            },
        ),
        "reopened" => ("issue", "reopened this".into_any()),
        "merged" => (
            "merge",
            match sha {
                Some(s) => view! { "merged this as "<Sha sha=s/> }.into_any(),
                None => "merged this into the folder".into_any(),
            },
        ),
        "renamed" => (
            "pencil",
            view! { "changed the title from "<del>{from.unwrap_or_default()}</del>" to "{to.unwrap_or_default()} }.into_any(),
        ),
        "labeled" => (
            "label",
            view! { "added the label "<strong>{label.unwrap_or_default()}</strong> }.into_any(),
        ),
        "unlabeled" => (
            "label",
            view! { "removed the label "<strong>{label.unwrap_or_default()}</strong> }.into_any(),
        ),
        "referenced" => (
            "link",
            view! {
                "mentioned this in "{ref_btn(number.unwrap_or(0))}
                {title.map(|t| view! { <span class="git-muted">{format!(" {t}")}</span> })}
            }.into_any(),
        ),
        "pushed" => (
            "upload",
            view! { "pushed to the branch: "<Sha sha=from.unwrap_or_default()/>" \u{2192} "<Sha sha=to.unwrap_or_default()/> }.into_any(),
        ),
        _ => (
            "branch",
            view! { "deleted the branch "<code>{from.unwrap_or_default()}</code>", which closed this" }.into_any(),
        ),
    };
    let time_title = local_date_time(
        created_at,
        &[("dateStyle", "medium"), ("timeStyle", "short")],
    );
    view! {
        <div class=format!("git-event is-{event}")>
            <Icon name=icon/>
            <span>
                <strong>{author}</strong>
                " "
                {body}
                " "
                <span class="git-muted" title=time_title>{edited(created_at)}</span>
            </span>
        </div>
    }
}

#[component]
fn Comment(
    id: String,
    author: String,
    body: String,
    created_at: f64,
    edited_at: Option<f64>,
    can_edit: bool,
    can_delete: bool,
    repo: GitRepository,
    session: BrowserSession,
    on_open: Callback<u32>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let editing = RwSignal::new(false);
    let text = RwSignal::new(body.clone());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let confirm = RwSignal::new(false);

    let url = StoredValue::new(format!("{}/comments/{id}", repo_api(&repo.id)));
    let csrf = StoredValue::new(session.csrf_token);
    let body_sv = StoredValue::new(body.clone());
    let author_sv = StoredValue::new(author.clone());

    let save = move |_| {
        busy.set(true);
        error.set(String::new());
        let u = url.get_value();
        let token = csrf.get_value();
        let payload = json!({ "body": text.get_untracked() });
        spawn_local(async move {
            match api::send::<Value>(&u, "PATCH", Some(payload), &token).await {
                Ok(_) => {
                    let _ = editing.try_set(false);
                    let _ = on_changed.try_run(());
                }
                Err(cause) => {
                    let _ = error.try_set(tracker_failure(&cause.message, "issue"));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let remove = move |_| {
        busy.set(true);
        error.set(String::new());
        let u = url.get_value();
        let token = csrf.get_value();
        spawn_local(async move {
            match api::send::<Value>(&u, "DELETE", None, &token).await {
                Ok(_) => {
                    let _ = confirm.try_set(false);
                    let _ = on_changed.try_run(());
                }
                Err(cause) => {
                    let _ = error.try_set(tracker_failure(&cause.message, "issue"));
                    let _ = busy.try_set(false);
                }
            }
        });
    };

    let has_actions = can_edit || can_delete;
    let menu_actions = Callback::new(move |_: ()| {
        let mut list = Vec::new();
        if can_edit {
            list.push(MenuAction::new("Edit", "pencil").run(move || {
                text.set(body_sv.get_value());
                editing.set(true);
            }));
        }
        if can_delete {
            let mut del = MenuAction::new("Delete", "trash")
                .danger()
                .run(move || confirm.set(true));
            if can_edit {
                del = del.separator();
            }
            list.push(del);
        }
        list
    });

    let time_title = local_date_time(
        created_at,
        &[("dateStyle", "medium"), ("timeStyle", "short")],
    );
    let time_label = format!(
        "{}{}",
        edited(created_at),
        if edited_at.is_some() {
            " \u{00b7} edited"
        } else {
            ""
        }
    );
    let aria_label = format!("Comment by {author}");
    let menu_label = format!("Actions for the comment by {author}");

    view! {
        <article class="git-comment" aria-label=aria_label>
            <header>
                <Person name=author.clone()/>
                <strong>{author}</strong>
                <span class="git-muted" title=time_title>{time_label}</span>
                {move || (has_actions && !editing.get()).then(|| view! {
                    <ActionMenu label=menu_label.clone() actions=menu_actions>
                        <Icon name="more"/>
                    </ActionMenu>
                })}
            </header>
            {move || if editing.get() {
                view! {
                    <div class="git-comment-body">
                        <MarkdownField label="Edit comment" value=text on_open=on_open autofocus=true rows=5/>
                        {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                        <div class="git-composer-actions">
                            <button type="button" class="text-button" on:click=move |_| editing.set(false)>"Cancel"</button>
                            <button type="button" class="primary-button" disabled=move || busy.get() || text.with(|t| t.trim().is_empty()) on:click=save>"Save"</button>
                        </div>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="git-comment-body">
                        <GitMarkdown source=body_sv.get_value() on_reference=on_open/>
                        {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                    </div>
                }.into_any()
            }}
            {move || confirm.get().then(|| view! {
                <Dialog title="Delete this comment?" on_close=Callback::new(move |_| confirm.set(false))>
                    <p>{format!("The comment by {} is removed from the conversation for everyone.", author_sv.get_value())}</p>
                    {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                    <div class="dialog-actions">
                        <button class="text-button" on:click=move |_| confirm.set(false)>"Cancel"</button>
                        <button class="danger-button" disabled=move || busy.get() on:click=remove>"Delete comment"</button>
                    </div>
                </Dialog>
            })}
        </article>
    }
}

#[component]
fn MarkdownField(
    #[prop(into)] label: String,
    value: RwSignal<String>,
    #[prop(optional)] on_open: Option<Callback<u32>>,
    #[prop(optional, into)] placeholder: Option<String>,
    #[prop(optional)] autofocus: bool,
    #[prop(default = 5)] rows: u32,
) -> impl IntoView {
    let mode = RwSignal::new("write".to_owned());
    let id = unique_id();
    let ta_ref = NodeRef::<html::Textarea>::new();
    let options = Signal::derive(|| {
        vec![
            ("write".to_owned(), "Write".to_owned()),
            ("preview".to_owned(), "Preview".to_owned()),
        ]
    });
    let view_label = format!("{label} view");
    let preview_label = format!("{label} preview");
    let id_body = id.clone();

    let apply_format = Callback::new(move |(before, after): (&'static str, &'static str)| {
        let Some(ta) = ta_ref.get_untracked() else {
            return;
        };
        let current = value.get_untracked();
        let start = ta
            .selection_start()
            .ok()
            .flatten()
            .unwrap_or_else(|| current.encode_utf16().count() as u32);
        let end = ta.selection_end().ok().flatten().unwrap_or(start);
        let (next, sel_start, sel_end) = format_markdown(&current, start, end, before, after);
        value.set(next.clone());
        ta.set_value(&next);
        let _ = ta.focus();
        let _ = ta.set_selection_range(sel_start, sel_end);
    });

    view! {
        <div class="git-markdown-field">
            <div class="git-markdown-bar">
                <label for=id.clone()>{label}</label>
                {move || (mode.get() == "write").then(|| view! {
                    <MarkdownToolbar on_format=apply_format disabled=Signal::derive(|| false)>
                        {()}
                    </MarkdownToolbar>
                })}
                <GitSegmented
                    label=view_label
                    value=mode
                    options=options
                    on_change=Callback::new(move |v| mode.set(v))
                />
            </div>
            {move || if mode.get() == "write" {
                view! {
                    <textarea
                        node_ref=ta_ref
                        id=id_body.clone()
                        prop:value=move || value.get()
                        rows=rows.to_string()
                        maxlength=TRACKER_LIMITS_BODY.to_string()
                        placeholder=placeholder.clone()
                        autofocus=autofocus
                        on:input=move |e| value.set(event_target_value(&e))
                    />
                }.into_any()
            } else {
                view! {
                    <div class="git-markdown-preview" id=id_body.clone() tabindex="0" aria-label=preview_label.clone()>
                        {move || {
                            let v = value.get();
                            if !v.trim().is_empty() {
                                if let Some(cb) = on_open {
                                    view! { <GitMarkdown source=v on_reference=cb/> }.into_any()
                                } else {
                                    view! { <GitMarkdown source=v/> }.into_any()
                                }
                            } else {
                                view! { <p class="git-muted">"Nothing to preview."</p> }.into_any()
                            }
                        }}
                    </div>
                }.into_any()
            }}
            <small class="git-muted">"Markdown. #12 links an issue or pull request."</small>
        </div>
    }
}

#[component]
fn Composer(
    issue: GitIssue,
    repo: GitRepository,
    session: BrowserSession,
    #[prop(into)] busy: Signal<bool>,
    on_open: Callback<u32>,
    on_close: Callback<&'static str>,
    on_commented: Callback<()>,
) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let sending = RwSignal::new(false);
    let error = RwSignal::new(String::new());

    let url = StoredValue::new(format!(
        "{}/issues/{}/comments",
        repo_api(&repo.id),
        issue.number
    ));
    let csrf = StoredValue::new(session.csrf_token);
    let kind = StoredValue::new(issue.kind.clone());
    let can_close = issue.permissions.close;
    let is_open = issue.state == "open";
    let is_closed = issue.state == "closed";

    let send = move |close_after: bool| {
        sending.set(true);
        error.set(String::new());
        let body_text = text.get_untracked();
        let u = url.get_value();
        let token = csrf.get_value();
        let k = kind.get_value();
        spawn_local(async move {
            let res = if !body_text.trim().is_empty() {
                api::send::<Value>(&u, "POST", Some(json!({ "body": body_text })), &token)
                    .await
                    .map(|_| ())
            } else {
                Ok(())
            };
            match res {
                Ok(()) => {
                    let _ = text.try_set(String::new());
                    let _ = on_commented.try_run(());
                    if close_after {
                        let _ = on_close.try_run("closed");
                    }
                }
                Err(cause) => {
                    let _ = error.try_set(tracker_failure(&cause.message, &k));
                }
            }
            let _ = sending.try_set(false);
        });
    };

    let close_icon = if issue.kind == "pull" {
        "pullClosed"
    } else {
        "issueDone"
    };
    let reopen_icon = if issue.kind == "pull" {
        "pull"
    } else {
        "issue"
    };
    let item_noun = noun(&issue.kind);

    view! {
        <section class="git-composer" aria-label="New comment">
            <MarkdownField label="Add a comment" value=text on_open=on_open placeholder="Leave a comment" rows=4/>
            {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
            <div class="git-composer-actions">
                {can_close.then(|| {
                    if is_open {
                        view! {
                            <button
                                type="button"
                                class="secondary-button"
                                disabled=move || busy.get() || sending.get()
                                on:click=move |_| send(true)
                            >
                                <Icon name=close_icon/>
                                {move || if !text.with(|t| t.trim().is_empty()) {
                                    "Close with comment".to_owned()
                                } else {
                                    format!("Close {item_noun}")
                                }}
                            </button>
                        }.into_any()
                    } else if is_closed {
                        view! {
                            <button
                                type="button"
                                class="secondary-button"
                                disabled=move || busy.get() || sending.get()
                                on:click=move |_| { let _ = on_close.try_run("open"); }
                            >
                                <Icon name=reopen_icon/>
                                "Reopen"
                            </button>
                        }.into_any()
                    } else {
                        ().into_any()
                    }
                })}
                <button
                    type="button"
                    class="primary-button"
                    disabled=move || sending.get() || text.with(|t| t.trim().is_empty())
                    on:click=move |_| send(false)
                >
                    <Icon name="comment"/>
                    "Comment"
                </button>
            </div>
        </section>
    }
}

#[derive(Deserialize)]
struct AgentMergeResp {
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    conflicts: Vec<String>,
}

#[component]
fn AgentMerge(
    repo: GitRepository,
    issue: GitIssue,
    agent: GitPullAgent,
    session: BrowserSession,
    on_merged: Callback<()>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());

    let url = StoredValue::new(format!("/api/v1/files/history/{}/branches/merge", repo.id));
    let head = StoredValue::new(issue.head.clone().unwrap_or_default());
    let revision = agent.revision;
    let csrf = StoredValue::new(session.csrf_token);
    let change_count = agent.changes.len();

    let do_merge = move |_| {
        busy.set(true);
        error.set(String::new());
        let u = url.get_value();
        let token = csrf.get_value();
        let body = json!({ "name": head.get_value(), "revision": revision });
        spawn_local(async move {
            match api::send::<AgentMergeResp>(&u, "POST", Some(body), &token).await {
                Ok(res) if res.merged => {
                    let _ = on_merged.try_run(());
                }
                Ok(res) => {
                    let prefix = if res.conflicts.len() == 1 {
                        "This file changed"
                    } else {
                        "These files changed"
                    };
                    let list = res
                        .conflicts
                        .iter()
                        .take(6)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    let _ = error.try_set(format!("{prefix} in the folder since Agent started: {list}. Ask Agent to update its changes."));
                    let _ = busy.try_set(false);
                }
                Err(cause) => {
                    let msg = if cause.message.contains("conflict") {
                        "The folder changed meanwhile. Reload and try again.".to_owned()
                    } else {
                        cause.message
                    };
                    let _ = error.try_set(msg);
                    let _ = busy.try_set(false);
                }
            }
        });
    };

    view! {
        <section class="git-merge" aria-label=format!("Merge {} into the folder", head.get_value())>
            <h3 class="git-sheet-label">"Merge into main"</h3>
            <p class="git-hint">{format!("{} from Agent change in the folder and are committed to main. Files changed since Agent started stop the merge.", plural(change_count, "file", None))}</p>
            {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
            <button
                type="button"
                class="primary-button git-merge-button"
                disabled=move || busy.get() || change_count == 0
                on:click=do_merge
            >
                <Icon name="merge"/>
                {move || if busy.get() { "Merging\u{2026}" } else { "Merge into main" }}
            </button>
        </section>
    }
}

// ---- Editing panes ------------------------------------------------------------------------------

#[component]
pub fn IssueEditor(
    repo: GitRepository,
    kind: String,
    existing: Option<GitIssue>,
    initial_head: Option<String>,
    initial_base: Option<String>,
    session: BrowserSession,
    on_close: Callback<()>,
    on_saved: Callback<u32>,
) -> impl IntoView {
    let pull = kind == "pull";
    let is_existing = existing.is_some();
    let orig_title = StoredValue::new(
        existing
            .as_ref()
            .map(|e| e.title.clone())
            .unwrap_or_default(),
    );
    let orig_body = StoredValue::new(
        existing
            .as_ref()
            .map(|e| e.body.clone())
            .unwrap_or_default(),
    );
    let existing_sv = StoredValue::new(existing.clone());
    let repo_sv = StoredValue::new(repo.clone());
    let kind_sv = StoredValue::new(kind.clone());
    let csrf = StoredValue::new(session.csrf_token);

    let title = RwSignal::new(orig_title.get_value());
    let body = RwSignal::new(orig_body.get_value());
    let chosen: RwSignal<Vec<String>> = RwSignal::new(
        existing
            .as_ref()
            .map(|e| e.labels.clone())
            .unwrap_or_default(),
    );
    let base = RwSignal::new(initial_base.unwrap_or_else(|| "main".to_owned()));
    let head = RwSignal::new(initial_head.unwrap_or_default());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let title_id = StoredValue::new(unique_id());

    let labels_url = Signal::derive(move || {
        Some(format!(
            "{}/labels",
            repo_api(&repo_sv.with_value(|r| r.id.clone()))
        ))
    });
    let labels_load = use_load::<Vec<GitLabel>>(labels_url, Signal::derive(|| 0u64));

    let branches_url = Signal::derive(move || {
        (pull && !is_existing).then(|| {
            format!(
                "{}/branches",
                repo_api(&repo_sv.with_value(|r| r.id.clone()))
            )
        })
    });
    let branches_load = use_load::<GitBranches>(branches_url, Signal::derive(|| 0u64));

    Effect::new(move |_| {
        if !pull || is_existing || !head.with_untracked(String::is_empty) {
            return;
        }
        if let Some(br) = branches_load.value.get() {
            let first = br
                .branches
                .first()
                .map(|b| b.name.clone())
                .or_else(|| br.agent.first().map(|a| a.name.clone()));
            if let Some(name) = first {
                head.set(name);
            }
        }
    });

    Effect::new(move |_| {
        if is_existing || !pull || !title.with_untracked(String::is_empty) {
            return;
        }
        let h = head.get();
        if let Some(br) = branches_load.value.get() {
            if let Some(ag) = br.agent.iter().find(|b| b.name == h)
                && !ag.message.is_empty()
            {
                title.set(ag.message.clone());
            } else if let Some(b) = br.branches.iter().find(|b| b.name == h) {
                title.set(b.subject.clone());
            }
        }
    });

    let compare_url = Signal::derive(move || {
        let h = head.get();
        let b = base.get();
        let is_agent = h.starts_with("agent/");
        (pull && !is_existing && !h.is_empty() && !is_agent && h != b).then(|| {
            format!(
                "{}/compare?{}",
                repo_api(&repo_sv.with_value(|r| r.id.clone())),
                query_string(&[("base", &b), ("head", &h)])
            )
        })
    });
    let compare_load = use_load::<GitCompare>(compare_url, Signal::derive(|| 0u64));

    let valid = Signal::derive(move || {
        let t = title.get();
        let ok_title = !t.trim().is_empty() && t.chars().count() <= TRACKER_LIMITS_TITLE;
        if !pull || is_existing {
            ok_title
        } else {
            let h = head.get();
            ok_title && !h.is_empty() && h != base.get()
        }
    });

    let dirty = Signal::derive(move || {
        !busy.get()
            && (title.get() != orig_title.get_value() || body.get() != orig_body.get_value())
    });

    let save = move || {
        busy.set(true);
        error.set(String::new());
        let t = title.get_untracked();
        let b = body.get_untracked();
        let ch = chosen.get_untracked();
        let token = csrf.get_value();
        let rid = repo_sv.with_value(|r| r.id.clone());
        let role_write = repo_sv.with_value(|r| r.role != "read");
        let k = kind_sv.get_value();
        let ex = existing_sv.get_value();
        let h = head.get_untracked();
        let bs = base.get_untracked();
        let is_agent = h.starts_with("agent/");

        spawn_local(async move {
            if let Some(ex) = ex {
                let labels_changed = role_write
                    && (ch.len() != ex.labels.len() || ch.iter().any(|id| !ex.labels.contains(id)));
                let mut map = Map::new();
                map.insert("revision".into(), ex.revision.into());
                map.insert("title".into(), t.into());
                map.insert("body".into(), b.into());
                if labels_changed {
                    map.insert("labels".into(), json!(ch));
                }
                let url = format!("{}/issues/{}", repo_api(&rid), ex.number);
                match api::send::<Value>(&url, "PATCH", Some(Value::Object(map)), &token).await {
                    Ok(_) => {
                        let _ = on_saved.try_run(ex.number);
                    }
                    Err(cause) => {
                        let _ = error.try_set(tracker_failure(&cause.message, &k));
                        let _ = busy.try_set(false);
                    }
                }
            } else {
                #[derive(Deserialize)]
                struct Created {
                    number: u32,
                }
                let mut map = Map::new();
                map.insert("kind".into(), k.clone().into());
                map.insert("title".into(), t.into());
                map.insert("body".into(), b.into());
                if !ch.is_empty() {
                    map.insert("labels".into(), json!(ch));
                }
                if pull {
                    map.insert(
                        "base".into(),
                        if is_agent { "main".into() } else { bs.into() },
                    );
                    map.insert("head".into(), h.into());
                }
                let url = format!("{}/issues", repo_api(&rid));
                match api::send::<Created>(&url, "POST", Some(Value::Object(map)), &token).await {
                    Ok(created) => {
                        let _ = on_saved.try_run(created.number);
                    }
                    Err(cause) => {
                        let _ = error.try_set(tracker_failure(&cause.message, &k));
                        let _ = busy.try_set(false);
                    }
                }
            }
        });
    };

    let heading = match &existing {
        Some(ex) => format!("Edit {} #{}", noun(&kind), ex.number),
        None => format!("New {}", noun(&kind)),
    };

    let base_options = Signal::derive(move || {
        branches_load.value.with(|br| match br {
            Some(b) => {
                let mut list = Vec::new();
                if b.main.is_some() {
                    list.push(("main".to_owned(), "main".to_owned()));
                }
                for item in &b.branches {
                    list.push((item.name.clone(), item.name.clone()));
                }
                list
            }
            None => Vec::new(),
        })
    });

    let head_options = Signal::derive(move || {
        let mut list = Vec::new();
        if head.with(String::is_empty) {
            list.push((String::new(), "Choose a branch".to_owned()));
        }
        branches_load.value.with(|br| {
            if let Some(b) = br {
                for item in &b.branches {
                    if item.name != "main" {
                        list.push((item.name.clone(), item.name.clone()));
                    }
                }
                for ag in &b.agent {
                    list.push((ag.name.clone(), format!("{} (Agent)", ag.name)));
                }
            }
        });
        list
    });

    let is_agent_sig = Signal::derive(move || head.with(|h| h.starts_with("agent/")));
    let effective_base = Signal::derive(move || {
        if is_agent_sig.get() {
            "main".to_owned()
        } else {
            base.get()
        }
    });
    let role_write = repo.role != "read";

    view! {
        <EditorPane title=heading on_close=on_close dirty=dirty>
            <form
                class="git-new git-issue-form"
                on:submit=move |e: web_sys::SubmitEvent| {
                    e.prevent_default();
                    if valid.get_untracked() && !busy.get_untracked() {
                        save();
                    }
                }
            >
                {(pull && !is_existing).then(|| view! {
                    <section class="git-form-section" aria-label="Branches">
                        <h3 class="git-sheet-label"><Icon name="pull"/>"Branches"</h3>
                        {move || {
                            if branches_load.value.with(Option::is_none) {
                                return view! { <Loading label="Loading branches"/> }.into_any();
                            }
                            view! {
                                <div class="git-compare-pick">
                                    <GitSelect
                                        aria_label="Branch to merge"
                                        value=head
                                        options=head_options
                                        on_change=Callback::new(move |v| head.set(v))
                                    />
                                    <Icon name="arrowRight"/>
                                    <GitSelect
                                        aria_label="Merge into"
                                        value=effective_base
                                        options=base_options
                                        disabled=is_agent_sig
                                        on_change=Callback::new(move |v| base.set(v))
                                    />
                                </div>
                                {move || {
                                    let h = head.get();
                                    let b = base.get();
                                    if h.is_empty() {
                                        view! { <p class="git-hint">"Push a branch with Git, or let Agent work on this folder, to open a pull request."</p> }.into_any()
                                    } else if h.starts_with("agent/") {
                                        view! { <p class="git-hint">"Agent\u{2019}s changes merge into main through Agent\u{2019}s merge, which checks every file first."</p> }.into_any()
                                    } else if h == b {
                                        view! { <p class="git-hint">"Choose two different branches."</p> }.into_any()
                                    } else if !compare_load.error.with(String::is_empty) {
                                        view! { <p class="error" role="alert">{tracker_failure(&compare_load.error.get(), "pull")}</p> }.into_any()
                                    } else if let Some(cmp) = compare_load.value.get() {
                                        if cmp.ahead.is_empty() {
                                            view! {
                                                <div class="git-banner is-info">
                                                    <Icon name="info"/>
                                                    <div>
                                                        <strong>"Nothing to merge yet"</strong>
                                                        <span>{format!("{b} already has every commit on {h}. You can still open the pull request and push to the branch.")}</span>
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else {
                                            let cls = if cmp.mergeable { "git-pull-check is-ok" } else { "git-pull-check is-conflict" };
                                            let ic = if cmp.mergeable { "check" } else { "alert" };
                                            let status_str = if cmp.mergeable {
                                                if cmp.fast_forward { "Can fast-forward".to_owned() } else { "Can merge".to_owned() }
                                            } else {
                                                format!("Conflicts with {b}")
                                            };
                                            view! {
                                                <p class=cls>
                                                    <Icon name=ic/>
                                                    {format!("{} \u{00b7} {} \u{00b7} {status_str}", plural(cmp.ahead.len(), "commit", None), plural(cmp.files.len(), "file", None))}
                                                </p>
                                            }.into_any()
                                        }
                                    } else {
                                        view! { <Loading label="Comparing"/> }.into_any()
                                    }
                                }}
                            }.into_any()
                        }}
                    </section>
                })}
                <label class="git-field" for=title_id.get_value()>
                    "Title"
                    <input
                        id=title_id.get_value()
                        prop:value=move || title.get()
                        maxlength=TRACKER_LIMITS_TITLE.to_string()
                        required
                        autofocus
                        on:input=move |e| title.set(event_target_value(&e))
                    />
                </label>
                <MarkdownField
                    label="Description"
                    value=body
                    rows=8
                    placeholder=if pull {
                        "What this changes and why. \u{201c}Closes #12\u{201d} closes that issue when this is merged."
                    } else {
                        "What happens, and what should happen instead."
                    }
                />
                {role_write.then(|| view! {
                    <fieldset class="git-form-section git-label-choices">
                        <legend class="git-sheet-label"><Icon name="label"/>"Labels"</legend>
                        {move || match labels_load.value.get() {
                            None => view! { <Loading label="Loading labels"/> }.into_any(),
                            Some(list) if list.is_empty() => view! { <p class="git-hint">"No labels yet. Create them from Labels in the Issues tab."</p> }.into_any(),
                            Some(list) => list.into_iter().map(|lbl| {
                                let id_chk = lbl.id.clone();
                                let id_dis = lbl.id.clone();
                                let id_chg = lbl.id.clone();
                                let desc = lbl.description.clone();
                                view! {
                                    <label class="git-check">
                                        <input
                                            type="checkbox"
                                            prop:checked=move || chosen.with(|c| c.contains(&id_chk))
                                            disabled=move || chosen.with(|c| !c.contains(&id_dis) && c.len() >= TRACKER_LIMITS_LABELS_PER_ITEM)
                                            on:change=move |e| {
                                                let checked = event_target_checked(&e);
                                                let id = id_chg.clone();
                                                chosen.update(|c| {
                                                    if checked {
                                                        if !c.contains(&id) { c.push(id); }
                                                    } else {
                                                        c.retain(|x| x != &id);
                                                    }
                                                });
                                            }
                                        />
                                        <LabelChip label=lbl/>
                                        {(!desc.is_empty()).then(|| view! { <small class="git-muted">{desc}</small> })}
                                    </label>
                                }
                            }).collect_view().into_any(),
                        }}
                    </fieldset>
                })}
                {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                <div class="git-new-actions">
                    <button type="button" class="text-button" on:click=move |_| { let _ = on_close.try_run(()); }>"Cancel"</button>
                    <button type="submit" class="primary-button" disabled=move || !valid.get() || busy.get()>
                        {move || if busy.get() {
                            "Saving\u{2026}"
                        } else if is_existing {
                            "Save"
                        } else if pull {
                            "Open pull request"
                        } else {
                            "Open issue"
                        }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}

#[component]
pub fn LabelsEditor(
    repo: GitRepository,
    session: BrowserSession,
    on_close: Callback<()>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let repo_sv = StoredValue::new(repo.clone());
    let csrf = StoredValue::new(session.csrf_token);
    let url = Signal::derive(move || {
        Some(format!(
            "{}/labels",
            repo_api(&repo_sv.with_value(|r| r.id.clone()))
        ))
    });
    let loaded = use_load::<Vec<GitLabel>>(url, Signal::derive(|| 0u64));

    let edit_id: RwSignal<Option<String>> = RwSignal::new(None);
    let name = RwSignal::new(String::new());
    let color = RwSignal::new("plum".to_owned());
    let description = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let failure = RwSignal::new(String::new());
    let remove_target: RwSignal<Option<GitLabel>> = RwSignal::new(None);
    let name_id = StoredValue::new(unique_id());
    let desc_id = StoredValue::new(unique_id());

    let reload_labels = move || {
        let u = format!("{}/labels", repo_api(&repo_sv.with_value(|r| r.id.clone())));
        spawn_local(async move {
            if let Ok(list) = api::get::<Vec<GitLabel>>(&u).await {
                loaded.set(list);
            }
            let _ = on_changed.try_run(());
        });
    };

    let valid = Signal::derive(move || {
        let n = name.get();
        !n.trim().is_empty() && n.chars().count() <= TRACKER_LIMITS_LABEL_NAME
    });
    let dirty = Signal::derive(move || !name.with(String::is_empty) && !busy.get());

    let save = move || {
        busy.set(true);
        failure.set(String::new());
        let id = edit_id.get_untracked();
        let body = json!({
            "name": name.get_untracked(),
            "color": color.get_untracked(),
            "description": description.get_untracked(),
        });
        let base_url = format!("{}/labels", repo_api(&repo_sv.with_value(|r| r.id.clone())));
        let (target_url, method) = match id {
            Some(ref i) => (format!("{base_url}/{i}"), "PATCH"),
            None => (base_url, "POST"),
        };
        let token = csrf.get_value();
        spawn_local(async move {
            match api::send::<Value>(&target_url, method, Some(body), &token).await {
                Ok(_) => {
                    let _ = edit_id.try_set(None);
                    let _ = name.try_set(String::new());
                    let _ = description.try_set(String::new());
                    reload_labels();
                }
                Err(cause) => {
                    let _ = failure.try_set(tracker_failure(&cause.message, "label"));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let confirm_remove = move |lbl: GitLabel| {
        busy.set(true);
        failure.set(String::new());
        let target_url = format!(
            "{}/labels/{}",
            repo_api(&repo_sv.with_value(|r| r.id.clone())),
            lbl.id
        );
        let token = csrf.get_value();
        let lbl_id = lbl.id;
        spawn_local(async move {
            match api::send::<Value>(&target_url, "DELETE", None, &token).await {
                Ok(_) => {
                    let _ = remove_target.try_set(None);
                    if edit_id.try_get_untracked().flatten().as_deref() == Some(lbl_id.as_str()) {
                        let _ = edit_id.try_set(None);
                        let _ = name.try_set(String::new());
                        let _ = color.try_set("plum".to_owned());
                        let _ = description.try_set(String::new());
                    }
                    reload_labels();
                }
                Err(cause) => {
                    let _ = failure.try_set(tracker_failure(&cause.message, "label"));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        <EditorPane title="Labels" on_close=on_close dirty=dirty>
            <form
                class="git-new"
                on:submit=move |e: web_sys::SubmitEvent| {
                    e.prevent_default();
                    if valid.get_untracked() && !busy.get_untracked() {
                        save();
                    }
                }
            >
                <section class="git-form-section" aria-label=move || if edit_id.with(Option::is_some) { "Edit label" } else { "New label" }>
                    <h3 class="git-sheet-label">{move || if edit_id.with(Option::is_some) { "Edit label" } else { "New label" }}</h3>
                    <label class="git-field" for=name_id.get_value()>
                        "Name"
                        <input
                            id=name_id.get_value()
                            prop:value=move || name.get()
                            maxlength=TRACKER_LIMITS_LABEL_NAME.to_string()
                            on:input=move |e| name.set(event_target_value(&e))
                        />
                    </label>
                    <label class="git-field" for=desc_id.get_value()>
                        "Description"
                        <input
                            id=desc_id.get_value()
                            prop:value=move || description.get()
                            maxlength=TRACKER_LIMITS_LABEL_DESCRIPTION.to_string()
                            on:input=move |e| description.set(event_target_value(&e))
                        />
                    </label>
                    <fieldset class="git-colors">
                        <legend>"Colour"</legend>
                        {GIT_LABEL_COLORS.into_iter().map(|c| {
                            let cap = format!("{}{}", c[..1].to_uppercase(), &c[1..]);
                            view! {
                                <label class=move || {
                                    if color.get() == c {
                                        format!("git-color is-{c} is-checked")
                                    } else {
                                        format!("git-color is-{c}")
                                    }
                                }>
                                    <input
                                        type="radio"
                                        name="label-color"
                                        value=c
                                        prop:checked=move || color.get() == c
                                        on:change=move |_| color.set(c.to_owned())
                                    />
                                    <span class="git-color-dot" aria-hidden="true"></span>
                                    <span>{cap}</span>
                                </label>
                            }
                        }).collect_view()}
                    </fieldset>
                    <p class="git-label-preview">
                        "Preview "
                        {move || {
                            let n = name.get();
                            let trimmed = n.trim();
                            let preview = GitLabel {
                                id: String::new(),
                                name: if trimmed.is_empty() { "label".to_owned() } else { trimmed.to_owned() },
                                color: color.get(),
                                description: description.get(),
                            };
                            view! { <LabelChip label=preview/> }
                        }}
                    </p>
                    {move || (!failure.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{failure.get()}</p> })}
                    <div class="git-new-actions">
                        {move || edit_id.with(Option::is_some).then(|| view! {
                            <button
                                type="button"
                                class="text-button"
                                on:click=move |_| {
                                    edit_id.set(None);
                                    name.set(String::new());
                                    color.set("plum".to_owned());
                                    description.set(String::new());
                                }
                            >
                                "Cancel editing"
                            </button>
                        })}
                        <button type="submit" class="primary-button" disabled=move || !valid.get() || busy.get()>
                            {move || if edit_id.with(Option::is_some) { "Save label" } else { "Create label" }}
                        </button>
                    </div>
                </section>
                <section class="git-form-section" aria-label="Labels in this repository">
                    <h3 class="git-sheet-label">
                        {move || match loaded.value.get() {
                            Some(list) => plural(list.len(), "label", None),
                            None => "Labels".to_owned(),
                        }}
                    </h3>
                    {move || {
                        if !loaded.error.with(String::is_empty) {
                            return view! { <Failure error=loaded.error retry=loaded.retry what="Labels"/> }.into_any();
                        }
                        let Some(list) = loaded.value.get() else {
                            return view! { <Loading label="Loading labels"/> }.into_any();
                        };
                        if list.is_empty() {
                            return view! { <p class="git-hint">"Labels sort issues and pull requests, such as bug or docs."</p> }.into_any();
                        }
                        view! {
                            <ul class="git-label-list">
                                {list.into_iter().map(|lbl| {
                                    let menu_label = format!("Actions for {}", lbl.name);
                                    let desc = lbl.description.clone();
                                    let lbl_edit = lbl.clone();
                                    let lbl_del = lbl.clone();
                                    let acts = Callback::new(move |_: ()| {
                                        let e = lbl_edit.clone();
                                        let d = lbl_del.clone();
                                        vec![
                                            MenuAction::new("Edit", "pencil").run(move || {
                                                edit_id.set(Some(e.id.clone()));
                                                name.set(e.name.clone());
                                                color.set(e.color.clone());
                                                description.set(e.description.clone());
                                            }),
                                            MenuAction::new("Delete", "trash").danger().separator().run(move || remove_target.set(Some(d.clone()))),
                                        ]
                                    });
                                    view! {
                                        <li>
                                            <LabelChip label=lbl/>
                                            <span class="git-muted">{desc}</span>
                                            <ActionMenu label=menu_label actions=acts>
                                                <Icon name="more"/>
                                            </ActionMenu>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }}
                </section>
            </form>
            {move || remove_target.get().map(|lbl| {
                let lbl_sv = StoredValue::new(lbl.clone());
                let title = format!("Delete {}?", lbl.name);
                view! {
                    <Dialog title=title on_close=Callback::new(move |_| remove_target.set(None))>
                        <p>{format!(
                            "The label is removed from every issue and pull request in {}. Their history keeps a note that it was there.",
                            repo_sv.with_value(|r| r.name.clone())
                        )}</p>
                        {move || (!failure.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{failure.get()}</p> })}
                        <div class="dialog-actions">
                            <button class="text-button" on:click=move |_| remove_target.set(None)>"Cancel"</button>
                            <button class="danger-button" disabled=move || busy.get() on:click=move |_| confirm_remove(lbl_sv.get_value())>"Delete label"</button>
                        </div>
                    </Dialog>
                }
            })}
        </EditorPane>
    }
}
