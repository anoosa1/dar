//! Repository Branches, Tags, and Compare sheet with inline diffs (`BranchesTab`, `TagsTab`, `CompareSheet`).

use std::collections::{HashMap, HashSet};

use leptos::html;
use leptos::prelude::*;

use super::diff::DiffMode;
use super::model::{
    GitBranch, GitBranches, GitCompare, GitIssueList, GitMergeResult, GitRepository, GitTag,
    RefLists, focus_sheet, plural, query_string, repo_api, sheet_keydown, use_load, zip_url,
};
use super::parts::{
    Failure, FileChanges, GitSegmented, GitSelect, Loading, MergeBox, Person, Sha, SheetHead, copy,
};
use crate::api::BrowserSession;
use crate::dates::edited;
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction};

#[component]
pub fn BranchesTab(
    repo: GitRepository,
    refs: RwSignal<Option<RefLists>>,
    write: bool,
    tracker_version: RwSignal<u64>,
    on_pull: Callback<String>,
    on_open_issue: Callback<u32>,
    on_compare: Callback<String>,
    on_commit: Callback<String>,
    on_delete: Callback<GitBranch>,
    on_code: Callback<String>,
    notify: Callback<String>,
) -> impl IntoView {
    let repo_id = repo.id.clone();
    let pulls_url = {
        let id = repo.id.clone();
        Signal::derive(move || {
            let _ = tracker_version.get();
            Some(format!("{}/issues?kind=pull&state=open", repo_api(&id)))
        })
    };
    let pulls_load = use_load::<GitIssueList>(pulls_url, tracker_version.into());

    let pr_by_head = Memo::new(move |_| {
        let mut map: HashMap<String, u32> = HashMap::new();
        pulls_load.value.with(|maybe| {
            if let Some(list) = maybe {
                for item in &list.items {
                    if let Some(ref head) = item.head {
                        map.entry(head.clone()).or_insert(item.number);
                    }
                }
            }
        });
        map
    });

    view! {
        {move || {
            let Some(lists) = refs.get() else {
                return view! { <div class="git-scroll"><Loading label="Loading branches"/></div> }.into_any();
            };
            let GitBranches { main, branches, agent } = lists.branches;
            let pr_map = pr_by_head.get();
            let widest = branches
                .iter()
                .flat_map(|b| [b.ahead, b.behind])
                .max()
                .unwrap_or(1)
                .max(1) as f64;
            let bar = move |value: usize| {
                let w = if value == 0 {
                    0
                } else {
                    ((value as f64 / widest * 10.0).round() as usize).max(1)
                };
                format!("w{w}")
            };

            let render_row = |branch: GitBranch, is_main: bool| {
                let repo_id = repo_id.clone();
                let sha = branch.sha.clone();
                let branch_name = branch.name.clone();
                let compare_name = branch.name.clone();
                let open_pr = (!is_main).then(|| pr_map.get(&branch.name).copied()).flatten();
                let menu_label = format!("Actions for {}", branch.name);
                let branch_for_menu = branch.clone();
                let actions = Callback::new(move |_| {
                    let b = branch_for_menu.clone();
                    let mut items: Vec<MenuAction> = Vec::new();
                    let code_name = if is_main { String::new() } else { b.name.clone() };
                    items.push(MenuAction::new("Browse files", "code").run(move || { let _ = on_code.try_run(code_name.clone()); }));
                    if !is_main {
                        let cmp = b.name.clone();
                        items.push(MenuAction::new("Compare with main", "compare").run(move || { let _ = on_compare.try_run(cmp.clone()); }));
                    }
                    if !is_main && write {
                        if let Some(pr_num) = open_pr {
                            items.push(MenuAction::new(format!("View pull request #{pr_num}"), "pull").run(move || { let _ = on_open_issue.try_run(pr_num); }));
                        } else {
                            let pr = b.name.clone();
                            items.push(MenuAction::new("Open pull request", "pull").run(move || { let _ = on_pull.try_run(pr.clone()); }));
                        }
                    }
                    let mut zip = MenuAction::new("Download ZIP", "download").href(zip_url(&repo_id, &b.name));
                    zip.download = true;
                    items.push(zip);
                    let copy_name = b.name.clone();
                    items.push(MenuAction::new("Copy name", "copy").run(move || copy(copy_name.clone(), notify, "branch name")));
                    if !is_main && write {
                        let del_branch = b.clone();
                        let mut del = MenuAction::new("Delete branch", "trash").run(move || { let _ = on_delete.try_run(del_branch.clone()); });
                        del.danger = true;
                        del.separator = true;
                        items.push(del);
                    }
                    items
                });
                let meta_tail = format!(" \u{00b7} {} \u{00b7} {}", branch.author, edited(branch.time));
                let div_label = format!("{} behind and {} ahead of main", branch.behind, branch.ahead);
                view! {
                    <li class="git-branch-row">
                        <Icon name="branch"/>
                        <div class="git-branch-body">
                            <span class="git-branch-name">
                                {branch_name}
                                {is_main.then(|| view! { <span class="git-tag">"Default"</span> })}
                                {open_pr.map(|num| view! {
                                    <button
                                        type="button"
                                        class="git-pr-pill"
                                        on:click=move |_| { let _ = on_open_issue.try_run(num); }
                                    >
                                        <Icon name="pull"/>
                                        {format!("PR #{num} Open")}
                                    </button>
                                })}
                            </span>
                            <span>
                                <button type="button" class="git-link" on:click=move |_| { let _ = on_commit.try_run(sha.clone()); }>{branch.subject}</button>
                                {meta_tail}
                            </span>
                        </div>
                        {if is_main {
                            view! { <span class="git-muted git-branch-state">"The folder matches this branch"</span> }.into_any()
                        } else {
                            view! {
                                <span class="git-divergence" role="img" aria-label=div_label>
                                    <span class="is-behind">
                                        <small>{format!("{} behind", branch.behind)}</small>
                                        <i class=bar(branch.behind)></i>
                                    </span>
                                    <span class="is-ahead">
                                        <small>{format!("{} ahead", branch.ahead)}</small>
                                        <i class=bar(branch.ahead)></i>
                                    </span>
                                </span>
                            }.into_any()
                        }}
                        <span class="git-branch-actions">
                            {(!is_main).then(|| view! {
                                <button type="button" class="secondary-button" on:click=move |_| { let _ = on_compare.try_run(compare_name.clone()); }>
                                    <Icon name="compare"/>
                                    "Compare"
                                </button>
                            })}
                            <ActionMenu label=menu_label actions=actions>
                                <Icon name="more"/>
                            </ActionMenu>
                        </span>
                    </li>
                }
            };

            view! {
                <div class="git-scroll git-branches">
                    <section class="git-group" aria-label="Default branch">
                        <h2>"Default branch"</h2>
                        {match main {
                            Some(m) => view! { <ul>{render_row(m, true)}</ul> }.into_any(),
                            None => view! { <p class="git-hint">"Nothing is committed on main yet."</p> }.into_any(),
                        }}
                    </section>
                    <section class="git-group" aria-label="Branches">
                        <h2>"Branches"</h2>
                        {if branches.is_empty() {
                            view! {
                                <p class="git-hint">
                                    "Push a branch with Git to keep work beside main: "
                                    <code>"git push origin my-branch"</code>
                                </p>
                            }.into_any()
                        } else {
                            view! { <ul>{branches.into_iter().map(|b| render_row(b, false)).collect_view()}</ul> }.into_any()
                        }}
                    </section>
                    {(!agent.is_empty()).then(|| view! {
                        <section class="git-group" aria-label="Agent branches">
                            <h2>"Agent branches"</h2>
                            <ul>
                                {agent.into_iter().map(|branch| {
                                    let pr_name = branch.name.clone();
                                    let open_pr = pr_map.get(&branch.name).copied();
                                    let msg = if branch.message.is_empty() { "No message".to_owned() } else { branch.message };
                                    let meta = format!("{} \u{00b7} {} \u{00b7} {}", msg, plural(branch.files, "file", None), edited(branch.modified_at));
                                    let href = format!("/agent?thread={}", super::model::encode_uri(&branch.thread_id));
                                    view! {
                                        <li class="git-branch-row">
                                            <Icon name="branch" class="is-agent"/>
                                            <div class="git-branch-body">
                                                <span class="git-branch-name">
                                                    {branch.name}
                                                    {open_pr.map(|num| view! {
                                                        <button
                                                            type="button"
                                                            class="git-pr-pill"
                                                            on:click=move |_| { let _ = on_open_issue.try_run(num); }
                                                        >
                                                            <Icon name="pull"/>
                                                            {format!("PR #{num} Open")}
                                                        </button>
                                                    })}
                                                </span>
                                                <span>{meta}</span>
                                            </div>
                                            <span></span>
                                            <span class="git-branch-actions">
                                                {write.then(|| {
                                                    if let Some(num) = open_pr {
                                                        view! {
                                                            <button type="button" class="secondary-button" on:click=move |_| { let _ = on_open_issue.try_run(num); }>
                                                                <Icon name="pull"/>
                                                                {format!("PR #{num}")}
                                                            </button>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <button type="button" class="secondary-button" on:click=move |_| { let _ = on_pull.try_run(pr_name.clone()); }>
                                                                <Icon name="pull"/>
                                                                "Open pull request"
                                                            </button>
                                                        }.into_any()
                                                    }
                                                })}
                                                <a class="secondary-button" href=href>
                                                    <Icon name="external"/>
                                                    "Open in Agent"
                                                </a>
                                            </span>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                            <p class="git-group-note">
                                <Icon name="info"/>
                                "Agent branches are reviewed in Agent, or in a pull request here, and merged into the folder. Pushes to agent/ branches are refused."
                            </p>
                        </section>
                    })}
                </div>
            }.into_any()
        }}
    }
}

#[component]
pub fn TagsTab(
    repo: GitRepository,
    refs: RwSignal<Option<RefLists>>,
    write: bool,
    on_commit: Callback<String>,
    on_code: Callback<String>,
    on_delete: Callback<GitTag>,
    notify: Callback<String>,
) -> impl IntoView {
    let repo_id = repo.id.clone();

    view! {
        {move || {
            let Some(lists) = refs.get() else {
                return view! { <div class="git-scroll"><Loading label="Loading tags"/></div> }.into_any();
            };
            if lists.tags.is_empty() {
                return view! {
                    <div class="git-empty">
                        <span class="empty-icon"><Icon name="tag"/></span>
                        <h2>"No tags yet"</h2>
                        <p>"Tag a version with Git and push it:"</p>
                        <pre class="git-command">"git tag -a v1.0 -m \"Version 1.0\"\ngit push origin v1.0"</pre>
                    </div>
                }.into_any();
            }
            let newest_idx = lists
                .tags
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.time.total_cmp(&b.time))
                .map(|(i, _)| i);
            view! {
                <ul class="git-scroll git-tag-list" aria-label="Tags">
                    {lists.tags.into_iter().enumerate().map(|(index, tag)| {
                        let is_latest = Some(index) == newest_idx;
                        let target_btn = tag.target.clone();
                        let target_sha = tag.target.clone();
                        let browse_name = tag.name.clone();
                        let zip_href = zip_url(&repo_id, &tag.name);
                        let label = format!("Actions for {}", tag.name);
                        let msg = if tag.message.is_empty() { "Lightweight tag".to_owned() } else { tag.message.clone() };
                        let meta = format!("{} \u{00b7} {} \u{00b7} {}", msg, tag.author, edited(tag.time));
                        let tag_for_menu = tag.clone();
                        let actions = Callback::new(move |_| {
                            let t = tag_for_menu.clone();
                            let code_name = t.name.clone();
                            let target_show = t.target.clone();
                            let copy_name = t.name.clone();
                            let mut items = vec![
                                MenuAction::new("Browse files", "code").run(move || { let _ = on_code.try_run(code_name.clone()); }),
                                MenuAction::new("Show commit", "commit").run(move || { let _ = on_commit.try_run(target_show.clone()); }),
                                MenuAction::new("Copy name", "copy").run(move || copy(copy_name.clone(), notify, "tag name")),
                            ];
                            if write {
                                let del_tag = t.clone();
                                let mut del = MenuAction::new("Delete tag", "trash").run(move || { let _ = on_delete.try_run(del_tag.clone()); });
                                del.danger = true;
                                del.separator = true;
                                items.push(del);
                            }
                            items
                        });
                        view! {
                            <li class="git-tag-row">
                                <Icon name="tag"/>
                                <div class="git-branch-body">
                                    <span class="git-branch-name">
                                        {tag.name}
                                        {is_latest.then(|| view! { <span class="git-tag is-latest">"Latest"</span> })}
                                    </span>
                                    <span>{meta}</span>
                                </div>
                                <button type="button" class="git-link" on:click=move |_| { let _ = on_commit.try_run(target_btn.clone()); }>
                                    <Sha sha=target_sha/>
                                </button>
                                <span class="git-branch-actions">
                                    <button type="button" class="secondary-button" on:click=move |_| { let _ = on_code.try_run(browse_name.clone()); }>
                                        <Icon name="code"/>
                                        "Browse"
                                    </button>
                                    <a class="secondary-button" href=zip_href download="">
                                        <Icon name="download"/>
                                        "ZIP"
                                    </a>
                                    <ActionMenu label=label actions=actions>
                                        <Icon name="more"/>
                                    </ActionMenu>
                                </span>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            }.into_any()
        }}
    }
}

#[component]
pub fn CompareSheet(
    repo: GitRepository,
    session: BrowserSession,
    diff_mode_str: RwSignal<String>,
    wrap: RwSignal<bool>,
    notify: Callback<String>,
    expanded: Signal<bool>,
    on_expand: Callback<()>,
    on_merged: Callback<()>,
    on_pull: Callback<(String, String)>,
    base: String,
    head: String,
    on_change: Callback<(String, String)>,
    on_commit: Callback<String>,
    on_close: Callback<()>,
) -> impl IntoView {
    let compare_url = format!(
        "{}/compare?{}",
        repo_api(&repo.id),
        query_string(&[("base", base.as_str()), ("head", head.as_str())])
    );
    let branches_url = format!("{}/branches", repo_api(&repo.id));
    let compare = use_load::<GitCompare>(
        Signal::derive(move || Some(compare_url.clone())),
        Signal::derive(|| 0u64),
    );
    let lists = use_load::<GitBranches>(
        Signal::derive(move || Some(branches_url.clone())),
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
    let root = NodeRef::<html::Div>::new();
    focus_sheet(root);

    let base_sv = StoredValue::new(base.clone());
    let head_sv = StoredValue::new(head.clone());
    let repo_sv = StoredValue::new(repo);
    let session_sv = StoredValue::new(session);

    let branch_options = Signal::derive(move || {
        let mut names: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        if let Some(data) = lists.value.get() {
            if data.main.is_some() && seen.insert("main".to_owned()) {
                names.push("main".to_owned());
            }
            for b in data.branches {
                if seen.insert(b.name.clone()) {
                    names.push(b.name);
                }
            }
        } else {
            for n in [base_sv.get_value(), head_sv.get_value()] {
                if seen.insert(n.clone()) {
                    names.push(n);
                }
            }
        }
        names
            .into_iter()
            .map(|n| (n.clone(), n))
            .collect::<Vec<_>>()
    });

    view! {
        <div
            node_ref=root
            class="git-sheet"
            tabindex="-1"
            on:keydown=move |ev| sheet_keydown(ev, on_close)
        >
            <SheetHead
                title=view! { "Compare" }.into_any()
                expanded=expanded
                on_expand=on_expand
                on_close=on_close
            />
            <div class="git-sheet-body">
                <div class="git-compare-pick">
                    <GitSelect
                        aria_label="Compared branch"
                        value=Signal::derive(move || head_sv.get_value())
                        options=branch_options
                        on_change=Callback::new(move |next_head| { let _ = on_change.try_run((base_sv.get_value(), next_head)); })
                    />
                    <Icon name="arrowRight"/>
                    <GitSelect
                        aria_label="Base branch"
                        value=Signal::derive(move || base_sv.get_value())
                        options=branch_options
                        on_change=Callback::new(move |next_base| { let _ = on_change.try_run((next_base, head_sv.get_value())); })
                    />
                </div>
                {move || {
                    if !compare.error.with(String::is_empty) {
                        return view! { <Failure error=compare.error retry=compare.retry what="The comparison"/> }.into_any();
                    }
                    let Some(value) = compare.value.get() else {
                        return view! { <Loading label="Comparing"/> }.into_any();
                    };
                    let b = base_sv.get_value();
                    let h = head_sv.get_value();
                    let r = repo_sv.get_value();
                    let can_write = r.role != "read";
                    let has_ahead = !value.ahead.is_empty();
                    let added_total: usize = value.files.iter().map(|f| f.added).sum();
                    let removed_total: usize = value.files.iter().map(|f| f.removed).sum();
                    let compare_for_merge = value.clone();
                    let b_pull = b.clone();
                    let h_pull = h.clone();
                    let b_merge = b.clone();
                    let h_merge = h.clone();

                    view! {
                        {if !has_ahead {
                            let behind_note = if value.behind > 0 {
                                format!(" It is {} behind.", plural(value.behind, "commit", None))
                            } else {
                                String::new()
                            };
                            view! {
                                <p class="git-hint">{format!("{h} has no commits that {b} doesn\u{2019}t.{behind_note}")}</p>
                            }.into_any()
                        } else {
                            view! {
                                <h3 class="git-sheet-label">{format!("{} on {h}", plural(value.ahead.len(), "commit", None))}</h3>
                                <ol class="git-mini-commits">
                                    {value.ahead.into_iter().map(|commit| {
                                        let sha = commit.sha.clone();
                                        view! {
                                            <li>
                                                <button type="button" on:click=move |_| { let _ = on_commit.try_run(sha.clone()); }>
                                                    <Person name=commit.author source=commit.source/>
                                                    <span>{commit.subject}</span>
                                                    <Sha sha=commit.sha/>
                                                    <small>{edited(commit.time)}</small>
                                                </button>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ol>
                                <div class="git-files-summary">
                                    <strong>{format!("{} changed", plural(value.files.len(), "file", None))}</strong>
                                    <span class="git-stat">
                                        <span class="git-added">{format!("+{added_total}")}</span>
                                        " "
                                        <span class="git-removed">{format!("\u{2212}{removed_total}")}</span>
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
                                <FileChanges files=value.files wrap=wrap mode=diff_mode/>
                            }.into_any()
                        }}
                        {(has_ahead && can_write).then(|| view! {
                            <button
                                type="button"
                                class="secondary-button git-open-pull"
                                on:click=move |_| { let _ = on_pull.try_run((b_pull.clone(), h_pull.clone())); }
                            >
                                <Icon name="pull"/>
                                "Open pull request"
                            </button>
                            <MergeBox
                                repo=r
                                compare=compare_for_merge
                                session=session_sv.get_value()
                                on_merged=Callback::new(move |result: GitMergeResult| {
                                    notify.run(if result.fast_forward {
                                        format!("{b_merge} now points at {h_merge}")
                                    } else {
                                        format!("Merged {h_merge} into {b_merge}")
                                    });
                                    let _ = on_merged.try_run(());
                                })
                            />
                        })}
                    }.into_any()
                }}
            </div>
        </div>
    }
}
