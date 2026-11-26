//! Repository Commits history (graph + grouped list) and Commit detail sheet (`Commits`, `CommitSheet`).

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use super::code::RefPicker;
use super::diff::DiffMode;
use super::graph::{GraphRow, graph_rows};
use super::model::{
    GitCommit, GitCommitDetail, GitCommitPage, GitPrefs, GitRepository, RefLists, focus_sheet,
    plural, query_string, repo_api, sheet_keydown, short, use_load, zip_url,
};
use super::parts::{
    Failure, FileChanges, GitSegmented, GitSelect, Loading, Person, Refs, Sha, SheetHead,
    SourceChip, copy,
};
use crate::api::{self, BrowserSession, Req};
use crate::dates::{edited, local_date, local_date_time};
use crate::icons::Icon;
use crate::ui::Dialog;

#[component]
pub fn Commits(
    repo: Signal<GitRepository>,
    refs: RwSignal<Option<RefLists>>,
    selected: Signal<Option<String>>,
    prefs: GitPrefs,
    on_commit: Callback<String>,
) -> impl IntoView {
    let git_ref = RwSignal::new("all".to_owned());
    let author = RwSignal::new(String::new());
    let source = RwSignal::new(String::new());
    let mode = RwSignal::new(prefs.commits_view.get_untracked());
    let pages: RwSignal<Vec<GitCommitPage>> = RwSignal::new(Vec::new());
    let error = RwSignal::new(String::new());
    let loading = RwSignal::new(false);
    let attempt = RwSignal::new(0u64);

    let build_params = move |skip: usize| {
        let r = git_ref.get_untracked();
        let a = author.get_untracked();
        let s = source.get_untracked();
        let skip_str = skip.to_string();
        let mut pairs: Vec<(&str, &str)> = vec![("ref", r.as_str()), ("skip", skip_str.as_str())];
        if !a.is_empty() {
            pairs.push(("author", a.as_str()));
        }
        if !s.is_empty() {
            pairs.push(("source", s.as_str()));
        }
        query_string(&pairs)
    };

    Effect::new(move |_| {
        let r = repo.get();
        let _rev = r.revision;
        let current_ref = git_ref.get();
        let current_author = author.get();
        let current_source = source.get();
        let _ = attempt.get();

        pages.set(Vec::new());
        error.set(String::new());
        loading.set(true);

        let mut pairs: Vec<(&str, &str)> = vec![("ref", current_ref.as_str()), ("skip", "0")];
        if !current_author.is_empty() {
            pairs.push(("author", current_author.as_str()));
        }
        if !current_source.is_empty() {
            pairs.push(("source", current_source.as_str()));
        }
        let url = format!("{}/commits?{}", repo_api(&r.id), query_string(&pairs));
        let abort = web_sys::AbortController::new().ok();
        let signal = abort.as_ref().map(|a| a.signal());
        if let Some(abort) = abort {
            let abort = send_wrapper::SendWrapper::new(abort);
            on_cleanup(move || abort.abort());
        }
        spawn_local(async move {
            let check = signal.clone();
            let res = api::request::<GitCommitPage>(
                &url,
                Req {
                    signal,
                    ..Default::default()
                },
            )
            .await;
            if check.as_ref().is_some_and(|s| s.aborted()) {
                return;
            }
            match res {
                Ok(page) => {
                    let _ = pages.try_set(vec![page]);
                }
                Err(cause) => {
                    let _ = error.try_set(cause.message);
                }
            }
            let _ = loading.try_set(false);
        });
    });

    let load_more = move |_| {
        let next = pages.with_untracked(|list| list.last().and_then(|p| p.next));
        let Some(next_skip) = next else { return };
        loading.set(true);
        let id = repo.with_untracked(|r| r.id.clone());
        let url = format!("{}/commits?{}", repo_api(&id), build_params(next_skip));
        spawn_local(async move {
            match api::get::<GitCommitPage>(&url).await {
                Ok(page) => {
                    let _ = pages.try_update(|list| list.push(page));
                }
                Err(cause) => {
                    let _ = error.try_set(cause.message);
                }
            }
            let _ = loading.try_set(false);
        });
    };

    let author_options = Signal::derive(move || {
        let mut out = vec![(String::new(), "All authors".to_owned())];
        pages.with(|list| {
            if let Some(first) = list.first() {
                for name in &first.authors {
                    out.push((name.clone(), name.clone()));
                }
            }
        });
        out
    });

    let source_options = Signal::derive(move || {
        vec![
            (String::new(), "All sources".to_owned()),
            ("git".to_owned(), "Git push".to_owned()),
            ("dar".to_owned(), "Dar".to_owned()),
            ("agent".to_owned(), "Agent".to_owned()),
        ]
    });

    let mode_options = Signal::derive(move || {
        vec![
            ("graph".to_owned(), "Graph".to_owned()),
            ("list".to_owned(), "List".to_owned()),
        ]
    });

    view! {
        <div class="git-scroll git-commits">
            <div class="git-toolbar">
                <RefPicker
                    refs=refs
                    value=Signal::derive(move || git_ref.get())
                    all=true
                    on_change=Callback::new(move |val: String| git_ref.set(if val.is_empty() { "all".to_owned() } else { val }))
                    label="Branch"
                />
                <GitSelect
                    aria_label="Author"
                    value=author
                    options=author_options
                    on_change=Callback::new(move |val| author.set(val))
                />
                <GitSelect
                    aria_label="Source"
                    value=source
                    options=source_options
                    on_change=Callback::new(move |val| source.set(val))
                />
                <span class="git-toolbar-end">
                    {move || pages.with(|list| list.first().map(|first| view! {
                        <span class="git-muted">{plural(first.total, "commit", None)}</span>
                    }))}
                    <GitSegmented
                        label="Commit view"
                        value=mode
                        options=mode_options
                        on_change=Callback::new(move |val| mode.set(val))
                    />
                </span>
            </div>
            {move || (mode.get() == "graph" && (!author.with(String::is_empty) || !source.with(String::is_empty))).then(|| view! {
                <p class="git-hint">"The graph shows every commit; filtered commits are listed instead."</p>
            })}
            {move || {
                if !error.with(String::is_empty) {
                    return view! {
                        <Failure error=error retry=Callback::new(move |_| attempt.update(|n| *n += 1)) what="Commits"/>
                    }.into_any();
                }
                let list = pages.get();
                if list.is_empty() {
                    return view! { <Loading label="Loading commits"/> }.into_any();
                }
                let commits: Vec<GitCommit> = list.into_iter().flat_map(|p| p.commits).collect();
                if commits.is_empty() {
                    return view! { <p class="git-hint">"No commits match."</p> }.into_any();
                }
                let use_graph = mode.get() == "graph"
                    && author.with(String::is_empty)
                    && source.with(String::is_empty);
                if use_graph {
                    let rows = graph_rows(&commits, |c| c.sha.as_str(), |c| c.parents.as_slice());
                    view! {
                        <ol class="git-graph" aria-label="Commits">
                            {commits.into_iter().zip(rows).map(|(commit, row)| {
                                let sha = commit.sha.clone();
                                let sha_click = commit.sha.clone();
                                view! {
                                    <li>
                                        <CommitRow
                                            commit=commit
                                            selected=Signal::derive(move || selected.get().as_deref() == Some(sha.as_str()))
                                            on_open=Callback::new(move |_| { let _ = on_commit.try_run(sha_click.clone()); })
                                            graph=row
                                        />
                                    </li>
                                }
                            }).collect_view()}
                        </ol>
                    }.into_any()
                } else {
                    view! { <CommitList commits=commits selected=selected on_commit=on_commit/> }.into_any()
                }
            }}
            {move || {
                let has_more = pages.with(|list| list.last().and_then(|p| p.next).is_some());
                has_more.then(|| view! {
                    <button class="secondary-button git-more" disabled=move || loading.get() on:click=load_more>
                        {move || if loading.get() { "Loading\u{2026}" } else { "Load more commits" }}
                    </button>
                })
            }}
        </div>
    }
}

#[component]
fn CommitList(
    commits: Vec<GitCommit>,
    selected: Signal<Option<String>>,
    on_commit: Callback<String>,
) -> impl IntoView {
    let mut groups: Vec<(String, Vec<GitCommit>)> = Vec::new();
    for commit in commits {
        let day = local_date(
            commit.time,
            &[
                ("weekday", "long"),
                ("month", "long"),
                ("day", "numeric"),
                ("year", "numeric"),
            ],
        );
        if groups.last().is_some_and(|(d, _)| d == &day) {
            groups.last_mut().unwrap().1.push(commit);
        } else {
            groups.push((day, vec![commit]));
        }
    }
    view! {
        <div class="git-commit-list">
            {groups.into_iter().map(|(day, items)| {
                let aria = day.clone();
                view! {
                    <section aria-label=aria>
                        <h3 class="git-day"><Icon name="commit"/>{day}</h3>
                        <ol>
                            {items.into_iter().map(|commit| {
                                let sha = commit.sha.clone();
                                let sha_click = commit.sha.clone();
                                view! {
                                    <li>
                                        <CommitRow
                                            commit=commit
                                            selected=Signal::derive(move || selected.get().as_deref() == Some(sha.as_str()))
                                            on_open=Callback::new(move |_| { let _ = on_commit.try_run(sha_click.clone()); })
                                        />
                                    </li>
                                }
                            }).collect_view()}
                        </ol>
                    </section>
                }
            }).collect_view()}
        </div>
    }
}

fn lane_x(lane: usize) -> f64 {
    9.0 + (lane as f64) * 14.0
}

const ROW_HEIGHT: f64 = 54.0;

/// One row of the graph: lines through the row, into the commit, and down to its parents.
#[component]
fn Lanes(row: GraphRow) -> impl IntoView {
    let x = lane_x(row.lane);
    let mid = ROW_HEIGHT / 2.0;
    let width = lane_x(row.width.max(1) - 1) + 9.0;
    let is_merge = row.out.len() > 1;
    let circle_class = if is_merge {
        format!("lane-{} is-merge", row.lane % 5)
    } else {
        format!("lane-{}", row.lane % 5)
    };
    let radius = if is_merge { 5.0 } else { 4.5 };

    view! {
        <svg
            class="git-lanes"
            width=width
            height=ROW_HEIGHT
            viewBox=format!("0 0 {width} {ROW_HEIGHT}")
            aria-hidden="true"
        >
            {row.through.into_iter().map(|lane| {
                view! { <path class=format!("lane-{}", lane % 5) d=format!("M{} 0V{ROW_HEIGHT}", lane_x(lane))/> }
            }).collect_view()}
            {row.top.then(|| {
                view! { <path class=format!("lane-{}", row.lane % 5) d=format!("M{x} 0V{mid}")/> }
            })}
            {row.into.into_iter().map(|lane| {
                let lx = lane_x(lane);
                view! { <path class=format!("lane-{}", lane % 5) d=format!("M{lx} 0C{lx} {mid} {x} {} {x} {mid}", mid * 0.4)/> }
            }).collect_view()}
            {row.out.into_iter().map(|lane| {
                let d = if lane == row.lane {
                    format!("M{x} {mid}V{ROW_HEIGHT}")
                } else {
                    let lx = lane_x(lane);
                    format!("M{x} {mid}C{x} {} {lx} {} {lx} {ROW_HEIGHT}", ROW_HEIGHT - 4.0, mid + 4.0)
                };
                view! { <path class=format!("lane-{}", lane % 5) d=d/> }
            }).collect_view()}
            <circle class=circle_class cx=x cy=mid r=radius/>
        </svg>
    }
}

#[component]
fn CommitRow(
    commit: GitCommit,
    selected: Signal<bool>,
    on_open: Callback<()>,
    #[prop(optional)] graph: Option<GraphRow>,
) -> impl IntoView {
    let has_graph = graph.is_some();
    let is_merge = commit.parents.len() > 1;
    let subject = if commit.subject.is_empty() {
        "(no message)".to_owned()
    } else {
        commit.subject
    };
    view! {
        <button
            type="button"
            class=move || {
                format!(
                    "git-commit{}{}",
                    if selected.get() { " is-selected" } else { "" },
                    if has_graph { " is-graph" } else { "" },
                )
            }
            aria-pressed=move || selected.get().to_string()
            on:click=move |_| { let _ = on_open.try_run(()); }
        >
            {match graph {
                Some(row) => view! { <Lanes row=row/> }.into_any(),
                None => view! { <Person name=commit.author.clone() source=commit.source.clone()/> }.into_any(),
            }}
            <span class="git-commit-main">
                <span class="git-commit-subject">
                    {is_merge.then(|| view! { <Icon name="merge" class="git-merge-icon"/> })}
                    <span class="git-commit-subj-text">{subject}</span>
                    <Refs refs=commit.refs/>
                </span>
                <span class="git-commit-sub">
                    {has_graph.then(|| view! { <Person name=commit.author.clone() source=commit.source.clone()/> })}
                    <strong>{commit.author}</strong>
                    <SourceChip source=commit.source/>
                    <span>{edited(commit.time)}</span>
                </span>
            </span>
            <span class="git-commit-end">
                <span class="git-stat">
                    <span class="git-added">{format!("+{}", commit.added)}</span>
                    " "
                    <span class="git-removed">{format!("\u{2212}{}", commit.removed)}</span>
                </span>
                <Sha sha=commit.sha/>
            </span>
        </button>
    }
}

#[component]
pub fn CommitSheet(
    repo: GitRepository,
    sha: String,
    session: BrowserSession,
    diff_mode_str: RwSignal<String>,
    wrap: RwSignal<bool>,
    expanded: Signal<bool>,
    on_expand: Callback<()>,
    on_changed: Callback<()>,
    on_close: Callback<()>,
    on_browse: Callback<String>,
    on_commit: Callback<String>,
    notify: Callback<String>,
) -> impl IntoView {
    let url = format!("{}/commits/{sha}", repo_api(&repo.id));
    let load = use_load::<GitCommitDetail>(
        Signal::derive(move || Some(url.clone())),
        Signal::derive(|| 0u64),
    );
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
    let restoring = RwSignal::new(false);
    let root = NodeRef::<html::Div>::new();
    focus_sheet(root);

    let sha_copy = sha.clone();
    let sha_browse = sha.clone();
    let zip_href = zip_url(&repo.id, &sha);
    let can_write = repo.role != "read";
    let repo_sv = StoredValue::new(repo);
    let session_sv = StoredValue::new(session);

    let title = view! { "Commit " <Sha sha=sha.clone()/> }.into_any();

    view! {
        <div
            node_ref=root
            class="git-sheet"
            tabindex="-1"
            on:keydown=move |ev| sheet_keydown(ev, on_close)
        >
            <SheetHead
                title=title
                expanded=expanded
                on_expand=on_expand
                on_close=on_close
            >
                <button
                    class="icon-button"
                    aria-label="Copy commit ID"
                    title="Copy commit ID"
                    on:click=move |_| copy(sha_copy.clone(), notify, "commit ID")
                >
                    <Icon name="copy"/>
                </button>
            </SheetHead>
            <div class="git-sheet-body">
                {move || {
                    if !load.error.with(String::is_empty) {
                        return view! { <Failure error=load.error retry=load.retry what="This commit"/> }.into_any();
                    }
                    let Some(commit) = load.value.get() else {
                        return view! { <Loading label="Loading commit"/> }.into_any();
                    };
                    let subject = if commit.subject.is_empty() {
                        "(no message)".to_owned()
                    } else {
                        commit.subject.clone()
                    };
                    let is_merge = commit.parents.len() > 1;
                    let parent_icon = if is_merge { "merge" } else { "history" };
                    let parent_label = if is_merge { "Parents" } else { "Parent" };
                    let has_refs = !commit.refs.is_empty();
                    let diff_committer = commit.committer != commit.author;
                    let file_count = commit.files.len();
                    let when_full = local_date_time(commit.time, &[("dateStyle", "medium"), ("timeStyle", "short")]);

                    view! {
                        <div>
                            <h3 class="git-commit-title">{subject}</h3>
                            {(!commit.body.is_empty()).then(|| view! { <p class="git-commit-body">{commit.body}</p> })}
                        </div>
                        <div class="git-who">
                            <Person name=commit.author.clone() source=commit.source.clone()/>
                            <div>
                                <span><strong>{commit.author.clone()}</strong>{format!(" committed {}", edited(commit.time))}</span>
                                <small>{format!("{when_full} \u{00b7} ")}<SourceChip source=commit.source/></small>
                            </div>
                        </div>
                        <div class="property-card git-facts">
                            <div class="property">
                                <Icon name="commit"/>
                                <div>
                                    <small>"Git ID"</small>
                                    <code class="git-full-sha">{commit.sha}</code>
                                </div>
                            </div>
                            <div class="property">
                                <Icon name=parent_icon/>
                                <div>
                                    <small>{parent_label}</small>
                                    <span>
                                        {if commit.parents.is_empty() {
                                            view! { "None, this is the first commit" }.into_any()
                                        } else {
                                            commit.parents.into_iter().enumerate().map(|(idx, parent)| {
                                                let p_click = parent.clone();
                                                view! {
                                                    <button type="button" class="git-link" on:click=move |_| { let _ = on_commit.try_run(p_click.clone()); }>
                                                        {(idx > 0).then_some(", ")}
                                                        <Sha sha=parent/>
                                                    </button>
                                                }
                                            }).collect_view().into_any()
                                        }}
                                    </span>
                                </div>
                            </div>
                            {has_refs.then(|| view! {
                                <div class="property">
                                    <Icon name="branch"/>
                                    <div>
                                        <small>"On"</small>
                                        <span class="git-refs"><Refs refs=commit.refs/></span>
                                    </div>
                                </div>
                            })}
                            {diff_committer.then(|| view! {
                                <div class="property">
                                    <Icon name="user"/>
                                    <div>
                                        <small>"Committed by"</small>
                                        <span>{format!("{}, {}", commit.committer, edited(commit.committed_at))}</span>
                                    </div>
                                </div>
                            })}
                        </div>
                        <div class="git-files-summary">
                            <strong>{format!("{} changed", plural(file_count, "file", None))}</strong>
                            {is_merge.then(|| view! { <span class="git-muted">"against the first parent"</span> })}
                            <span class="git-stat">
                                <span class="git-added">{format!("+{}", commit.added)}</span>
                                " "
                                <span class="git-removed">{format!("\u{2212}{}", commit.removed)}</span>
                            </span>
                            <span class="git-toolbar-end">
                                <GitSegmented
                                    label="Diff layout"
                                    value=diff_mode_str
                                    options=diff_options
                                    on_change=Callback::new(move |val| diff_mode_str.set(val))
                                />
                                <label class="git-wrap">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || wrap.get()
                                        on:change=move |ev| wrap.set(event_target_checked(&ev))
                                    />
                                    "Wrap lines"
                                </label>
                            </span>
                        </div>
                        <FileChanges files=commit.files wrap=wrap mode=diff_mode/>
                    }.into_any()
                }}
            </div>
            <div class="git-sheet-foot">
                <button class="secondary-button" on:click=move |_| { let _ = on_browse.try_run(sha_browse.clone()); }>
                    <Icon name="code"/>
                    "Browse files"
                </button>
                <a class="secondary-button" href=zip_href download="">
                    <Icon name="download"/>
                    "ZIP"
                </a>
                {move || {
                    let can_restore = can_write && load.value.with(|c| c.as_ref().and_then(|d| d.restore.as_ref()).is_some());
                    can_restore.then(|| view! {
                        <button class="secondary-button" on:click=move |_| restoring.set(true)>
                            <Icon name="history"/>
                            "Restore folder to this point"
                        </button>
                    })
                }}
            </div>
            {move || {
                if !restoring.get() {
                    return None;
                }
                let commit = load.value.get()?;
                commit.restore.as_ref()?;
                let commit_sv = StoredValue::new(commit);
                Some(view! {
                    <Dialog title="Restore this folder?" on_close=Callback::new(move |_| restoring.set(false))>
                        <RestoreFolder
                            repo=repo_sv.get_value()
                            commit=commit_sv.get_value()
                            session=session_sv.get_value()
                            on_cancel=Callback::new(move |_| restoring.set(false))
                            on_done=Callback::new(move |_| {
                                let subj = commit_sv.with_value(|c| c.subject.clone());
                                restoring.set(false);
                                notify.run(format!("Folder restored to \u{201c}{subj}\u{201d}"));
                                let _ = on_changed.try_run(());
                            })
                        />
                    </Dialog>
                })
            }}
        </div>
    }
}

/// Restores the live folder to a main commit; it is recorded as a new commit, so no history is lost.
#[component]
fn RestoreFolder(
    repo: GitRepository,
    commit: GitCommitDetail,
    session: BrowserSession,
    on_cancel: Callback<()>,
    on_done: Callback<()>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let restore = commit
        .restore
        .clone()
        .expect("RestoreFolder requires restore metadata");
    let repo_id = repo.id.clone();
    let csrf = session.csrf_token.clone();

    view! {
        <p>{format!(
            "The files in {} change back to how they were at \u{201c}{}\u{201d} ({}). This is recorded as a new commit on main, so nothing in the history is lost.",
            repo.path,
            commit.subject,
            short(&commit.sha),
        )}</p>
        {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
        <div class="dialog-actions">
            <button class="text-button" on:click=move |_| { let _ = on_cancel.try_run(()); }>"Cancel"</button>
            <button
                class="primary-button"
                disabled=move || busy.get()
                on:click=move |_| {
                    busy.set(true);
                    error.set(String::new());
                    let url = format!("/api/v1/files/history/{repo_id}/restore");
                    let body = json!({ "id": restore.changeset_id, "revision": restore.revision });
                    let token = csrf.clone();
                    spawn_local(async move {
                        match api::send::<serde_json::Value>(&url, "POST", Some(body), &token).await {
                            Ok(_) => { let _ = on_done.try_run(()); }
                            Err(cause) => {
                                let text = cause.message;
                                let friendly = if text.contains("uncommittedChanges") {
                                    "The folder has uncommitted changes. Commit them in Files first, then restore.".to_owned()
                                } else if text.contains("conflict") {
                                    "The history changed meanwhile. Close this and try again.".to_owned()
                                } else {
                                    text
                                };
                                let _ = error.try_set(friendly);
                                let _ = busy.try_set(false);
                            }
                        }
                    });
                }
            >
                "Restore folder"
            </button>
        </div>
    }
}
