//! Repository workspace (`Repository`, `CloneMenu`, `DeleteRef`), routing between the 6 tabs
//! and full-stage inline views (`FileStage`, full-stage PR review `IssueSheet`).

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use wasm_bindgen::JsCast;

use super::branches::{BranchesTab, TagsTab};
use super::code::{Code, FileStage};
use super::commits::Commits;
use super::model::{
    DeleteTarget, GitBranch, GitBranches, GitPrefs, GitRepository, GitTag, RefLists, Sheet, TABS,
    Tab, plural, repo_api, short, strip_ref_prefix, zip_url,
};
use super::parts::{Failure, RepoMark, copy};
use super::tracker::{IssueEditing, IssueSheet, IssuesTab};
use crate::api::{self, BrowserSession, Req};
use crate::icons::Icon;
use crate::ui::{ActionMenu, Dialog, MenuAction, document, listen, unique_id, use_floating};

#[component]
pub fn Repository(
    repo: Signal<GitRepository>,
    session: BrowserSession,
    tab: RwSignal<Tab>,
    on_tab: Callback<Tab>,
    git_ref: RwSignal<String>,
    on_ref: Callback<String>,
    path: RwSignal<String>,
    on_path: Callback<String>,
    sheet: RwSignal<Option<Sheet>>,
    on_sheet: Callback<Option<Sheet>>,
    prefs: GitPrefs,
    diff_mode_str: RwSignal<String>,
    wrap: RwSignal<bool>,
    focus_mode: RwSignal<bool>,
    notify: Callback<String>,
    on_changed: Callback<()>,
    tracker_version: RwSignal<u64>,
    issue_version: Signal<u64>,
    on_tracker_changed: Callback<()>,
    on_edit: Callback<IssueEditing>,
) -> impl IntoView {
    let version = RwSignal::new(0u64);
    let refs: RwSignal<Option<RefLists>> = RwSignal::new(None);
    let refs_error = RwSignal::new(String::new());
    let confirm: RwSignal<Option<DeleteTarget>> = RwSignal::new(None);
    let tab_ids = unique_id();
    let session_sv = StoredValue::new(session.clone());
    let repo_id_memo = Memo::new(move |_| repo.with(|r| r.id.clone()));

    Effect::new(move |_| {
        let r = repo.get();
        let _ = version.get();
        let id = r.id.clone();
        let _rev = r.revision;
        let abort = web_sys::AbortController::new().ok();
        let signal = abort.as_ref().map(|a| a.signal());
        if let Some(abort) = abort {
            let abort = send_wrapper::SendWrapper::new(abort);
            on_cleanup(move || abort.abort());
        }
        spawn_local(async move {
            let check = signal.clone();
            let branches_url = format!("{}/branches", repo_api(&id));
            let tags_url = format!("{}/tags", repo_api(&id));
            let branches_fut = api::request::<GitBranches>(
                &branches_url,
                Req {
                    signal: signal.clone(),
                    ..Default::default()
                },
            );
            let tags_fut = api::request::<Vec<GitTag>>(
                &tags_url,
                Req {
                    signal,
                    ..Default::default()
                },
            );
            match futures::try_join!(branches_fut, tags_fut) {
                Ok((branches, tags)) => {
                    if !check.as_ref().is_some_and(|s| s.aborted()) {
                        let _ = refs.try_set(Some(RefLists { branches, tags }));
                        let _ = refs_error.try_set(String::new());
                    }
                }
                Err(cause) => {
                    if !check.as_ref().is_some_and(|s| s.aborted()) {
                        let _ = refs_error.try_set(cause.message);
                    }
                }
            }
        });
    });

    let changed = Callback::new(move |_| {
        version.update(|n| *n += 1);
        let _ = on_changed.try_run(());
    });

    let pull_from = Callback::new(move |head: String| {
        let _ = on_edit.try_run(IssueEditing::Issue {
            kind: "pull".to_owned(),
            issue: None,
            base: Some("main".to_owned()),
            head: Some(head),
        });
    });

    let more_actions = Callback::new(move |_| {
        let r = repo.get_untracked();
        let files_href = if r.role == "owner" {
            format!("/?folder={}&file={}", r.parent_id, r.id)
        } else {
            format!("/?folder={}", r.id)
        };
        let clone_alias = r.clone.alias.clone();
        let mut bundle = MenuAction::new("Download Git bundle", "archive")
            .href(format!("/api/v1/files/history/{}/bundle", r.id));
        bundle.download = true;
        bundle.separator = true;
        vec![
            MenuAction::new("Show in Files", "folder").href(files_href),
            MenuAction::new("Copy clone address", "copy")
                .run(move || copy(clone_alias.clone(), notify, "clone address")),
            bundle,
        ]
    });

    // Separate full-stage overlays (`Sheet::File` and full-stage PR `Sheet::Issue { is_pull: true }`)
    // from repo row reloads so background counter refreshes never reset an open PR conversation or draft.
    let full_stage = Memo::new(move |_| match sheet.get() {
        Some(Sheet::File { git_ref, path }) => Some(Sheet::File { git_ref, path }),
        Some(Sheet::Issue {
            number,
            is_pull: true,
        }) => Some(Sheet::Issue {
            number,
            is_pull: true,
        }),
        _ => None,
    });

    let tab_ids_list = tab_ids.clone();
    let tab_ids_panel = tab_ids.clone();

    view! {
        <div class="git-repo">
            <div class="git-repo-head">
                {move || {
                    let r = repo.get();
                    let crumb_href = if r.role == "owner" {
                        format!("/?folder={}&file={}", r.parent_id, r.id)
                    } else {
                        format!("/?folder={}", r.id)
                    };
                    let crumb_icon = if r.role == "owner" { "folder" } else { "share" };
                    let crumb_text = if r.role == "owner" {
                        r.path.clone()
                    } else {
                        format!(
                            "Shared by {} \u{00b7} {}",
                            r.owner.as_deref().unwrap_or(""),
                            if r.role == "read" { "read only" } else { "can write" }
                        )
                    };
                    view! {
                        <RepoMark name=r.name.clone() size="is-m"/>
                        <div class="git-repo-title">
                            <h1>{r.name}</h1>
                            <a class="git-crumb" href=crumb_href>
                                <Icon name=crumb_icon/>
                                {crumb_text}
                            </a>
                        </div>
                    }
                }}
                <div class="git-tools">
                    <CloneMenu
                        repo=repo
                        git_ref=Signal::derive(move || {
                            let v = git_ref.get();
                            if v.is_empty() { "main".to_owned() } else { v }
                        })
                        username=session.user.username.clone()
                        notify=notify
                    />
                    <ActionMenu
                        label=Signal::derive(move || format!("More actions for {}", repo.with(|r| r.name.clone())))
                        actions=more_actions
                    >
                        <Icon name="more"/>
                    </ActionMenu>
                </div>
            </div>
            <div class="git-tabs" role="tablist" aria-label=move || format!("{} sections", repo.with(|r| r.name.clone()))>
                {TABS.into_iter().map(|t| {
                    let tab_id = format!("{tab_ids_list}-{}", t.as_str());
                    let controls = format!("{tab_ids_list}-panel");
                    view! {
                        <button
                            id=tab_id
                            role="tab"
                            aria-selected=move || (tab.get() == t).to_string()
                            aria-controls=controls
                            class=move || if tab.get() == t { "is-active" } else { "" }
                            on:click=move |_| { let _ = on_tab.try_run(t); }
                        >
                            <Icon name=t.icon()/>
                            {t.label()}
                            {move || {
                                let r = repo.get();
                                let count = match t {
                                    Tab::Code => None,
                                    Tab::Issues => Some(r.open_issues),
                                    Tab::Pulls => Some(r.open_pulls),
                                    Tab::Commits => Some(r.commits),
                                    Tab::Branches => Some(r.branches),
                                    Tab::Tags => Some(r.tags),
                                };
                                count.map(|c| view! { <span class="git-tab-count">{c}</span> })
                            }}
                        </button>
                    }
                }).collect_view()}
            </div>
            <div
                class="git-panel"
                role="tabpanel"
                id=format!("{tab_ids_panel}-panel")
                aria-labelledby=move || format!("{tab_ids_panel}-{}", tab.get().as_str())
            >
                {move || (!refs_error.with(String::is_empty)).then(|| view! {
                    <Failure
                        error=refs_error
                        retry=Callback::new(move |_| version.update(|n| *n += 1))
                        what="Branches and tags"
                    />
                })}
                {move || {
                    if let Some(stage) = full_stage.get() {
                        let _id = repo_id_memo.get();
                        let r = repo.get_untracked();
                        return match stage {
                            Sheet::File { git_ref: fref, path: fpath } => view! {
                                <FileStage
                                    repo=r
                                    git_ref=fref
                                    path=fpath
                                    wrap=wrap
                                    expanded=focus_mode.into()
                                    on_expand=Callback::new(move |_| focus_mode.update(|v| *v = !*v))
                                    on_select_file=Callback::new(move |(next_ref, next_path)| {
                                        let _ = on_sheet.try_run(Some(Sheet::File {
                                            git_ref: next_ref,
                                            path: next_path,
                                        }));
                                    })
                                    on_browse_dir=Callback::new(move |(next_ref, dir_path)| {
                                        let _ = on_ref.try_run(if next_ref == "main" { String::new() } else { next_ref });
                                        let _ = on_path.try_run(dir_path);
                                        let _ = on_sheet.try_run(None);
                                    })
                                    on_close=Callback::new(move |_| {
                                        focus_mode.set(false);
                                        let _ = on_sheet.try_run(None);
                                    })
                                    notify=notify
                                />
                            }.into_any(),
                            Sheet::Issue { number, .. } => view! {
                                <IssueSheet
                                    repo=r
                                    number=number
                                    session=session_sv.get_value()
                                    version=issue_version
                                    diff_mode_str=diff_mode_str
                                    wrap=wrap
                                    full_stage=true
                                    expanded=focus_mode.into()
                                    on_expand=Callback::new(move |_| focus_mode.update(|v| *v = !*v))
                                    on_kind_known=Callback::new(move |is_pull| {
                                        sheet.update(|cur| {
                                            if let Some(Sheet::Issue { number: n, is_pull: p }) = cur
                                                && *n == number
                                                && *p != is_pull
                                            {
                                                *p = is_pull;
                                            }
                                        });
                                    })
                                    notify=notify
                                    on_close=Callback::new(move |_| {
                                        focus_mode.set(false);
                                        let _ = on_sheet.try_run(None);
                                    })
                                    on_open=Callback::new(move |num| {
                                        let _ = on_sheet.try_run(Some(Sheet::Issue { number: num, is_pull: false }));
                                    })
                                    on_commit=Callback::new(move |sha| {
                                        focus_mode.set(false);
                                        let _ = on_sheet.try_run(Some(Sheet::Commit { sha }));
                                    })
                                    on_edit=on_edit
                                    on_changed=on_tracker_changed
                                />
                            }.into_any(),
                            _ => ().into_any(),
                        };
                    }

                    let current_tab = tab.get();
                    match current_tab {
                        Tab::Issues | Tab::Pulls => {
                            let r = repo.get();
                            let is_pull = current_tab == Tab::Pulls;
                            view! {
                                <IssuesTab
                                    repo=r
                                    kind=if is_pull { "pull" } else { "issue" }
                                    version=tracker_version
                                    selected=Signal::derive(move || match sheet.get() {
                                        Some(Sheet::Issue { number, .. }) => Some(number),
                                        _ => None,
                                    })
                                    on_open=Callback::new(move |number| {
                                        let _ = on_sheet.try_run(Some(Sheet::Issue { number, is_pull }));
                                    })
                                    on_edit=on_edit
                                />
                            }.into_any()
                        }
                        Tab::Code => view! {
                            <Code
                                repo=repo
                                refs=refs
                                git_ref=git_ref
                                on_ref=on_ref
                                path=path
                                on_path=on_path
                                on_file=Callback::new(move |file| {
                                    let current = git_ref.get_untracked();
                                    let _ = on_sheet.try_run(Some(Sheet::File {
                                        git_ref: if current.is_empty() { "main".to_owned() } else { current },
                                        path: file,
                                    }));
                                })
                                on_commits=Callback::new(move |_| { let _ = on_tab.try_run(Tab::Commits); })
                                on_commit=Callback::new(move |sha| { let _ = on_sheet.try_run(Some(Sheet::Commit { sha })); })
                                on_pulls=Callback::new(move |_| { let _ = on_tab.try_run(Tab::Pulls); })
                                on_branches=Callback::new(move |_| { let _ = on_tab.try_run(Tab::Branches); })
                                notify=notify
                                prefs=prefs
                            />
                        }.into_any(),
                        Tab::Commits => view! {
                            <Commits
                                repo=repo
                                refs=refs
                                selected=Signal::derive(move || match sheet.get() {
                                    Some(Sheet::Commit { sha }) => Some(sha),
                                    _ => None,
                                })
                                prefs=prefs
                                on_commit=Callback::new(move |sha| { let _ = on_sheet.try_run(Some(Sheet::Commit { sha })); })
                            />
                        }.into_any(),
                        Tab::Branches => {
                            let r = repo.get();
                            let write = r.role != "read";
                            view! {
                                <BranchesTab
                                    repo=r
                                    refs=refs
                                    write=write
                                    tracker_version=tracker_version
                                    on_pull=pull_from
                                    on_open_issue=Callback::new(move |num| {
                                        tab.set(Tab::Pulls);
                                        let _ = on_sheet.try_run(Some(Sheet::Issue { number: num, is_pull: true }));
                                    })
                                    on_compare=Callback::new(move |head| {
                                        let _ = on_sheet.try_run(Some(Sheet::Compare { base: "main".to_owned(), head }));
                                    })
                                    on_commit=Callback::new(move |sha| { let _ = on_sheet.try_run(Some(Sheet::Commit { sha })); })
                                    on_delete=Callback::new(move |branch: GitBranch| {
                                        confirm.set(Some(DeleteTarget {
                                            name: format!("refs/heads/{}", branch.name),
                                            old: branch.sha,
                                            kind: "branch",
                                            commits: Some(branch.ahead),
                                        }));
                                    })
                                    on_code=Callback::new(move |name| {
                                        let _ = on_ref.try_run(name);
                                        let _ = on_path.try_run(String::new());
                                        let _ = on_tab.try_run(Tab::Code);
                                    })
                                    notify=notify
                                />
                            }.into_any()
                        }
                        Tab::Tags => {
                            let r = repo.get();
                            let write = r.role != "read";
                            view! {
                                <TagsTab
                                    repo=r
                                    refs=refs
                                    write=write
                                    on_commit=Callback::new(move |sha| { let _ = on_sheet.try_run(Some(Sheet::Commit { sha })); })
                                    on_code=Callback::new(move |name| {
                                        let _ = on_ref.try_run(name);
                                        let _ = on_path.try_run(String::new());
                                        let _ = on_tab.try_run(Tab::Code);
                                    })
                                    on_delete=Callback::new(move |tag: GitTag| {
                                        confirm.set(Some(DeleteTarget {
                                            name: format!("refs/tags/{}", tag.name),
                                            old: tag.sha,
                                            kind: "tag",
                                            commits: None,
                                        }));
                                    })
                                    notify=notify
                                />
                            }.into_any()
                        }
                    }
                }}
            </div>
            {move || {
                let target = confirm.get()?;
                let short_name = strip_ref_prefix(&target.name).to_owned();
                let title = format!("Delete {short_name}?");
                let target_sv = StoredValue::new(target);
                let repo_sv = StoredValue::new(repo.get_untracked());
                Some(view! {
                    <Dialog title=title on_close=Callback::new(move |_| confirm.set(None))>
                        <DeleteRef
                            repo=repo_sv.get_value()
                            target=target_sv.get_value()
                            session=session_sv.get_value()
                            on_cancel=Callback::new(move |_| confirm.set(None))
                            on_deleted=Callback::new(move |_| {
                                let t = target_sv.get_value();
                                confirm.set(None);
                                notify.run(if t.kind == "tag" { "Tag deleted".to_owned() } else { "Branch deleted".to_owned() });
                                if git_ref.get_untracked() == strip_ref_prefix(&t.name) {
                                    let _ = on_ref.try_run(String::new());
                                }
                                if matches!(sheet.get_untracked(), Some(Sheet::Compare { .. })) {
                                    let _ = on_sheet.try_run(None);
                                }
                                changed.run(());
                            })
                        />
                    </Dialog>
                })
            }}
        </div>
    }
}

#[component]
fn DeleteRef(
    repo: GitRepository,
    target: DeleteTarget,
    session: BrowserSession,
    on_cancel: Callback<()>,
    on_deleted: Callback<()>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let name = strip_ref_prefix(&target.name).to_owned();
    let description = if target.kind == "tag" {
        format!(
            "The tag {name} is removed from {}. Commits it reached stay while a branch reaches them.",
            repo.name
        )
    } else if target.commits.unwrap_or(0) > 0 {
        format!(
            "{} only on {name}. They are removed from Dar and their storage is released. Clones that already have them keep them.",
            plural(
                target.commits.unwrap_or(0),
                "commit is",
                Some("commits are")
            )
        )
    } else {
        format!(
            "{name} is removed from {}. Its commits are also on main, so nothing else changes.",
            repo.name
        )
    };
    let repo_id = repo.id.clone();
    let full_name = target.name.clone();
    let old = target.old.clone();
    let kind = target.kind;
    let csrf = session.csrf_token.clone();

    view! {
        <p>{description}</p>
        {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
        <div class="dialog-actions">
            <button class="text-button" on:click=move |_| { let _ = on_cancel.try_run(()); }>"Cancel"</button>
            <button
                class="danger-button"
                disabled=move || busy.get()
                on:click=move |_| {
                    busy.set(true);
                    error.set(String::new());
                    let url = format!("{}/refs", repo_api(&repo_id));
                    let body = json!({ "name": full_name, "old": old });
                    let token = csrf.clone();
                    spawn_local(async move {
                        match api::send::<serde_json::Value>(&url, "DELETE", Some(body), &token).await {
                            Ok(_) => { let _ = on_deleted.try_run(()); }
                            Err(cause) => {
                                let _ = error.try_set(cause.message);
                                let _ = busy.try_set(false);
                            }
                        }
                    });
                }
            >
                {format!("Delete {kind}")}
            </button>
        </div>
    }
}

/// Clone popover: shows both alias and permanent HTTPS addresses simultaneously, downloads, and app-password link.
#[component]
fn CloneMenu(
    repo: Signal<GitRepository>,
    git_ref: Signal<String>,
    username: String,
    notify: Callback<String>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let trigger = NodeRef::<html::Button>::new();
    let panel = NodeRef::<html::Div>::new();
    let field_id = unique_id();
    let permanent_id = unique_id();
    let username_sv = StoredValue::new(username);

    use_floating(open.into(), trigger, panel, true, 6.0);

    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        if let Some(el) = panel.get()
            && let Ok(Some(input)) = el.query_selector("input")
            && let Ok(html) = input.dyn_into::<web_sys::HtmlElement>()
        {
            let _ = html.focus();
        }
        listen(&document(), "pointerdown", false, move |event| {
            let target = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            let in_panel = panel
                .get_untracked()
                .is_some_and(|p| p.contains(target.as_ref()));
            let in_trigger = trigger
                .get_untracked()
                .is_some_and(|t| t.contains(target.as_ref()));
            if !in_panel && !in_trigger {
                open.set(false);
            }
        });
    });

    let close = move || {
        open.set(false);
        if let Some(el) = trigger.get_untracked() {
            let _ = el.focus();
        }
    };

    view! {
        <button
            node_ref=trigger
            type="button"
            class="primary-button git-clone-button"
            aria-expanded=move || open.get().to_string()
            aria-haspopup="dialog"
            on:click=move |_| open.update(|v| *v = !*v)
        >
            <Icon name="download"/>
            "Clone"
            <Icon name="chevron"/>
        </button>
        {move || {
            open.get().then(|| {
                let r = repo.get();
                let current_ref = git_ref.get();
                let ref_label = if current_ref.len() == 40 { short(&current_ref) } else { current_ref.clone() };
                let alias_val = r.clone.alias.clone();
                let alias_copy = r.clone.alias.clone();
                let perm_val = r.clone.id.clone();
                let perm_copy = r.clone.id.clone();
                let zip_href = zip_url(&r.id, &current_ref);
                let bundle_href = format!("/api/v1/files/history/{}/bundle", r.id);
                let fid = field_id.clone();
                let fid_for = fid.clone();
                let pid = permanent_id.clone();
                let pid_for = pid.clone();
                let uname = username_sv.get_value();
                view! {
                    <div
                        node_ref=panel
                        class="git-clone-panel"
                        role="dialog"
                        aria-label="Clone"
                        on:keydown=move |event: web_sys::KeyboardEvent| {
                            if event.key() == "Escape" {
                                event.prevent_default();
                                event.stop_propagation();
                                close();
                            }
                        }
                    >
                        <label for=fid_for>"Clone with HTTPS"</label>
                        <span class="git-clone-row">
                            <input
                                id=fid
                                readonly=true
                                prop:value=alias_val
                                on:focus=move |event| {
                                    if let Some(input) = event.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                        input.select();
                                    }
                                }
                            />
                            <button
                                type="button"
                                class="icon-button"
                                aria-label="Copy clone address"
                                title="Copy"
                                on:click=move |_| copy(alias_copy.clone(), notify, "clone address")
                            >
                                <Icon name="copy"/>
                            </button>
                        </span>
                        <label for=pid_for>"Permanent address "<small>"keeps working after a rename or move"</small></label>
                        <span class="git-clone-row">
                            <input
                                id=pid
                                readonly=true
                                prop:value=perm_val
                                on:focus=move |event| {
                                    if let Some(input) = event.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                        input.select();
                                    }
                                }
                            />
                            <button
                                type="button"
                                class="icon-button"
                                aria-label="Copy permanent address"
                                title="Copy"
                                on:click=move |_| copy(perm_copy.clone(), notify, "permanent address")
                            >
                                <Icon name="copy"/>
                            </button>
                        </span>
                        <p>{format!("Sign in as {uname} with an app password that has Files access. Write access lets you push.")}</p>
                        <div class="git-clone-actions">
                            <a class="git-menu-row" href=zip_href download="">
                                <Icon name="download"/>
                                {format!("Download ZIP of {ref_label}")}
                            </a>
                            <a class="git-menu-row" href=bundle_href download="">
                                <Icon name="archive"/>
                                "Download Git bundle"
                                <small>"All branches and tags"</small>
                            </a>
                            <a class="git-menu-row" href="/settings?section=security">
                                <Icon name="key"/>
                                "Create app password"
                            </a>
                        </div>
                    </div>
                }
            })
        }}
    }
}
