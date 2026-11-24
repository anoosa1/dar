//! Git workspace (`next/client/git/workspace.tsx`): every Files folder that keeps history is a
//! repository. Overview shows a year of activity, the repositories and recent commits; a repository
//! has Code, Issues, Pull requests, Commits (graph or list), Branches and Tags, with commits,
//! comparisons, issues and files in the right sheet.

use std::collections::{HashMap, HashSet};

use js_sys::Date;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

use crate::api::{self, BrowserSession, Req, format_bytes};
use crate::dates::{edited, local_date, local_date_time};
use crate::files::picker::{FolderPicker, Selection};
use crate::git::graph::{GraphRow, graph_rows};
use crate::git::parts::{
    API, Failure, FileChanges, GitActivity, GitAgentBranch, GitBranch, GitBranches, GitCommit,
    GitCommitDetail, GitCommitPage, GitCompare, GitRef, GitRepository, GitSegmented, GitSelect,
    GitTag, GitTree, HistoryOverview, Loading, MergeBox, Person, Sha, SheetHead, SourceChip, copy,
    encode_uri, focus_sheet, kind_label, plural, query_string, repo_api, sheet_keydown, short,
    use_load,
};
use crate::git::tracker::{IssueEditing, IssueEditor, IssueSheet, IssuesTab, LabelsEditor};
use crate::icons::Icon;
use crate::live;
use crate::markdown::Markdown;
use crate::module_bindings::create_folder_reducer::create_folder;
use crate::random_uuid;
use crate::shell::WorkspaceHeader;
use crate::ui::{ActionMenu, Dialog, EditorPane, MenuAction, listen, unique_id, use_floating};

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    Repositories,
    Agent,
}

impl View {
    fn as_str(self) -> &'static str {
        match self {
            View::Overview => "overview",
            View::Repositories => "repositories",
            View::Agent => "agent",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "repositories" => View::Repositories,
            "agent" => View::Agent,
            _ => View::Overview,
        }
    }

    fn label(self) -> &'static str {
        match self {
            View::Overview => "Overview",
            View::Repositories => "Repositories",
            View::Agent => "Agent branches",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            View::Overview => "activity",
            View::Repositories => "repo",
            View::Agent => "agent",
        }
    }
}

const VIEWS: [View; 3] = [View::Overview, View::Repositories, View::Agent];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Code,
    Issues,
    Pulls,
    Commits,
    Branches,
    Tags,
}

impl Tab {
    fn as_str(self) -> &'static str {
        match self {
            Tab::Code => "code",
            Tab::Issues => "issues",
            Tab::Pulls => "pulls",
            Tab::Commits => "commits",
            Tab::Branches => "branches",
            Tab::Tags => "tags",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "issues" => Tab::Issues,
            "pulls" => Tab::Pulls,
            "commits" => Tab::Commits,
            "branches" => Tab::Branches,
            "tags" => Tab::Tags,
            _ => Tab::Code,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Code => "Code",
            Tab::Issues => "Issues",
            Tab::Pulls => "Pull requests",
            Tab::Commits => "Commits",
            Tab::Branches => "Branches",
            Tab::Tags => "Tags",
        }
    }

    fn icon(self) -> &'static str {
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

const TABS: [Tab; 6] = [
    Tab::Code,
    Tab::Issues,
    Tab::Pulls,
    Tab::Commits,
    Tab::Branches,
    Tab::Tags,
];

#[derive(Clone, PartialEq, Eq)]
enum Sheet {
    Commit { sha: String },
    Compare { base: String, head: String },
    File { git_ref: String, path: String },
    Issue { number: u32 },
}

#[derive(Clone)]
struct Notice {
    id: u64,
    message: String,
}

#[derive(Clone)]
struct RefLists {
    branches: GitBranches,
    tags: Vec<GitTag>,
}

#[derive(Clone)]
struct DeleteTarget {
    name: String,
    old: String,
    kind: &'static str,
    commits: Option<usize>,
}

/// Repository marks take one of a fixed set of hues from the name (colour is data, not theme).
fn hue(text: &str) -> usize {
    let mut h: u64 = 0;
    for ch in text.chars() {
        h = (h * 31 + ch as u64) % 9973;
    }
    (h % 6) as usize
}

fn zip_url(id: &str, git_ref: &str) -> String {
    format!("{}/archive?ref={}", repo_api(id), encode_uri(git_ref))
}

fn strip_ref_prefix(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/tags/"))
        .unwrap_or(name)
}

fn is_markdown_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

struct InitialState {
    view: View,
    repo: Option<String>,
    tab: Tab,
    git_ref: String,
    path: String,
    commit: Option<String>,
    issue: Option<u32>,
}

fn read_state() -> InitialState {
    let search = window().location().search().unwrap_or_default();
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok();
    let get = |key: &str| params.as_ref().and_then(|p| p.get(key));
    let view = get("view").map(|v| View::from_str(&v)).unwrap_or(View::Overview);
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
    InitialState {
        view,
        repo: get("repo"),
        tab,
        git_ref: get("ref").unwrap_or_default(),
        path: get("path").unwrap_or_default(),
        commit: get("commit"),
        issue,
    }
}

fn is_valid_folder_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && trimmed != "."
        && trimmed != ".."
        && name.chars().count() <= 255
}

/// Top-level Git workspace (`next/client/git/workspace.tsx` `GitWorkspace`).
#[component]
pub fn GitWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let start = read_state();
    let initial_sheet = if let Some(number) = start.issue {
        Some(Sheet::Issue { number })
    } else {
        start.commit.map(|sha| Sheet::Commit { sha })
    };

    let repos: RwSignal<Option<Vec<GitRepository>>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let reload = RwSignal::new(0u64);
    let view_mode = RwSignal::new(start.view);
    let repo_id: RwSignal<Option<String>> = RwSignal::new(start.repo);
    let tab = RwSignal::new(start.tab);
    let git_ref = RwSignal::new(start.git_ref);
    let path = RwSignal::new(start.path);
    let sheet: RwSignal<Option<Sheet>> = RwSignal::new(initial_sheet);
    let creating = RwSignal::new(false);
    let notice: RwSignal<Option<Notice>> = RwSignal::new(None);
    let notice_seq = RwSignal::new(0u64);
    let editing: RwSignal<Option<IssueEditing>> = RwSignal::new(None);
    let tracker_version = RwSignal::new(0u64);
    let navigation = RwSignal::new(false);
    let navigation_id = unique_id();

    let tracker_changed = Callback::new(move |_| {
        tracker_version.update(|n| *n += 1);
        reload.update(|n| *n += 1);
    });

    let load_seq = RwSignal::new(0u64);
    Effect::new(move |_| {
        let _ = reload.get();
        let seq = load_seq.get_untracked() + 1;
        load_seq.set(seq);
        let abort = web_sys::AbortController::new().ok();
        let signal = abort.as_ref().map(|a| a.signal());
        if let Some(abort) = abort {
            let abort = send_wrapper::SendWrapper::new(abort);
            on_cleanup(move || abort.abort());
        }
        spawn_local(async move {
            let check = signal.clone();
            let result = api::request::<Vec<GitRepository>>(
                &format!("{API}/repositories"),
                Req { signal, ..Default::default() },
            )
            .await;
            if load_seq.try_get_untracked() != Some(seq)
                || check.as_ref().is_some_and(|s| s.aborted())
            {
                return;
            }
            match result {
                Ok(list) => {
                    let _ = repos.try_set(Some(list));
                    let _ = error.try_set(String::new());
                }
                Err(cause) => {
                    let _ = error.try_set(cause.message);
                }
            }
        });
    });

    listen(&window(), "focus", false, move |_| {
        reload.update(|n| *n += 1);
    });

    // Keep URL search params synchronized with current workspace state.
    Effect::new(move |_| {
        let current_view = view_mode.get();
        let current_repo = repo_id.get();
        let current_tab = tab.get();
        let current_ref = git_ref.get();
        let current_path = path.get();
        let current_sheet = sheet.get();
        if let Ok(href) = window().location().href()
            && let Ok(url) = web_sys::Url::new(&href)
        {
            let params = url.search_params();
            for key in ["view", "repo", "tab", "ref", "path", "commit", "issue"] {
                params.delete(key);
            }
            if let Some(id) = current_repo {
                params.set("repo", &id);
                if current_tab != Tab::Code {
                    params.set("tab", current_tab.as_str());
                }
                if !current_ref.is_empty() {
                    params.set("ref", &current_ref);
                }
                if !current_path.is_empty() {
                    params.set("path", &current_path);
                }
                match current_sheet {
                    Some(Sheet::Commit { sha }) => params.set("commit", &sha),
                    Some(Sheet::Issue { number }) => params.set("issue", &number.to_string()),
                    _ => {}
                }
            } else if current_view != View::Overview {
                params.set("view", current_view.as_str());
            }
            if let Ok(history) = window().history() {
                let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url.href()));
            }
        }
    });

    // Clear toast notice after 5 seconds.
    Effect::new(move |_| {
        let Some(current) = notice.get() else {
            return;
        };
        let id = current.id;
        let handle = set_timeout_with_handle(
            move || {
                if notice.try_get_untracked().flatten().is_some_and(|n| n.id == id) {
                    let _ = notice.try_set(None);
                }
            },
            std::time::Duration::from_millis(5000),
        );
        if let Ok(handle) = handle {
            on_cleanup(move || handle.clear());
        }
    });

    let notify = Callback::new(move |text: String| {
        let next_id = notice_seq.get_untracked() + 1;
        notice_seq.set(next_id);
        notice.set(Some(Notice { id: next_id, message: text }));
    });

    let open_repo = Callback::new(move |(id, next_tab): (String, Tab)| {
        repo_id.set(Some(id));
        tab.set(next_tab);
        git_ref.set(String::new());
        path.set(String::new());
        sheet.set(None);
        editing.set(None);
        navigation.set(false);
    });

    let choose = Callback::new(move |next: View| {
        view_mode.set(next);
        repo_id.set(None);
        sheet.set(None);
        navigation.set(false);
    });

    let current_repo = Memo::new(move |_| {
        let id = repo_id.get()?;
        repos.with(|list| list.as_ref()?.iter().find(|item| item.id == id).cloned())
    });

    let agent_count = Memo::new(move |_| {
        repos.with(|list| {
            list.as_ref()
                .map(|items| items.iter().map(|i| i.agent_branches).sum::<usize>())
                .unwrap_or(0)
        })
    });

    let title = Memo::new(move |_| {
        if let Some(repo) = current_repo.get() {
            repo.name
        } else {
            view_mode.get().label().to_owned()
        }
    });

    // The issue sheet and the editing pane are keyed by the repository's id, not its row: a
    // tracker change reloads the list (open issue counts, `updatedAt`), which must not rebuild
    // an open conversation or form and lose its draft. The sheet's content follows
    // `tracker_version` and the repository's revision instead.
    let current_repo_id = Memo::new(move |_| current_repo.with(|r| r.as_ref().map(|r| r.id.clone())));
    let repo_revision = Memo::new(move |_| current_repo.with(|r| r.as_ref().map_or(0, |r| r.revision)));
    let issue_version = Signal::derive(move || tracker_version.get().wrapping_add(repo_revision.get()));
    let has_details = Memo::new(move |_| current_repo.get().is_some() && sheet.get().is_some());
    let session_sv = StoredValue::new(session.clone());
    let username = session.user.username.clone();

    view! {
        <div class=move || if has_details.get() { "app-shell pim-shell git-shell has-pim-details" } else { "app-shell pim-shell git-shell" }>
            <WorkspaceHeader app="git" session=session.clone() on_signed_out=on_signed_out/>
            <div class="pim-workspace git-workspace">
                {move || navigation.get().then(|| view! {
                    <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| navigation.set(false)></button>
                })}
                <aside
                    id=navigation_id
                    class=move || if navigation.get() { "sidebar git-sidebar is-open" } else { "sidebar git-sidebar" }
                    aria-label="Git navigation"
                >
                    <button type="button" class="new-button ui-new" on:click=move |_| creating.set(true)>
                        <Icon name="plus"/>
                        "New repository"
                    </button>
                    <nav aria-label="Git views">
                        {VIEWS.into_iter().map(|v| {
                            let active = move || repo_id.get().is_none() && view_mode.get() == v;
                            view! {
                                <button
                                    class=move || if active() { "nav-item active" } else { "nav-item" }
                                    aria-current=move || active().then_some("page")
                                    on:click=move |_| choose.run(v)
                                >
                                    <Icon name=v.icon()/>
                                    <span class="git-row-title">{v.label()}</span>
                                    {move || {
                                        if v == View::Repositories {
                                            let count = repos.with(|r| r.as_ref().map(Vec::len).unwrap_or(0));
                                            (count > 0).then(|| view! { <span class="git-count">{count}</span> })
                                        } else if v == View::Agent {
                                            let count = agent_count.get();
                                            (count > 0).then(|| view! { <span class="git-count">{count}</span> })
                                        } else {
                                            None
                                        }
                                    }}
                                </button>
                            }
                        }).collect_view()}
                    </nav>
                    <div class="sidebar-heading git-heading"><h2>"Repositories"</h2></div>
                    <nav aria-label="Repositories" class="git-repo-nav">
                        {move || {
                            let owned: Vec<GitRepository> = repos.with(|list| {
                                list.as_ref()
                                    .map(|items| items.iter().filter(|i| i.role == "owner").cloned().collect())
                                    .unwrap_or_default()
                            });
                            owned.into_iter().map(|item| {
                                let id_active = item.id.clone();
                                let id_aria = item.id.clone();
                                let id_click = item.id.clone();
                                let dirty = item.dirty;
                                view! {
                                    <button
                                        class=move || if repo_id.get().as_deref() == Some(id_active.as_str()) { "nav-item active" } else { "nav-item" }
                                        aria-current=move || (repo_id.get().as_deref() == Some(id_aria.as_str())).then_some("page")
                                        on:click=move |_| open_repo.run((id_click.clone(), Tab::Code))
                                    >
                                        <RepoMark name=item.name.clone()/>
                                        <span class="git-row-title">{item.name}</span>
                                        {(dirty > 0).then(|| {
                                            let text = plural(dirty as usize, "uncommitted change", None);
                                            let title_text = format!("{text} in Dar");
                                            view! {
                                                <span class="git-dirty-dot" title=title_text>
                                                    <span class="sr-only">{text}</span>
                                                </span>
                                            }
                                        })}
                                    </button>
                                }
                            }).collect_view()
                        }}
                    </nav>
                    {move || {
                        let shared: Vec<GitRepository> = repos.with(|list| {
                            list.as_ref()
                                .map(|items| items.iter().filter(|i| i.role != "owner").cloned().collect())
                                .unwrap_or_default()
                        });
                        (!shared.is_empty()).then(|| view! {
                            <div class="sidebar-heading git-heading"><h2>"Shared with you"</h2></div>
                            <nav aria-label="Shared repositories" class="git-repo-nav">
                                {shared.into_iter().map(|item| {
                                    let id_active = item.id.clone();
                                    let id_aria = item.id.clone();
                                    let id_click = item.id.clone();
                                    let role_label = if item.role == "read" { "Read" } else { "Write" };
                                    let shared_by = format!("Shared by {}", item.owner.as_deref().unwrap_or(""));
                                    view! {
                                        <button
                                            class=move || if repo_id.get().as_deref() == Some(id_active.as_str()) { "nav-item active" } else { "nav-item" }
                                            aria-current=move || (repo_id.get().as_deref() == Some(id_aria.as_str())).then_some("page")
                                            on:click=move |_| open_repo.run((id_click.clone(), Tab::Code))
                                        >
                                            <RepoMark name=item.name.clone()/>
                                            <span class="git-row-title">{item.name}</span>
                                            <span class="git-count" title=shared_by>{role_label}</span>
                                        </button>
                                    }
                                }).collect_view()}
                            </nav>
                        })
                    }}
                    <div class="git-key-card">
                        <Icon name="key"/>
                        <div>
                            <strong>"Clone with an app password"</strong>
                            <span>{format!("Sign in as {username} with an app password that has Files access.")}</span>
                            <a href="/settings?section=security">"Create app password"</a>
                        </div>
                    </div>
                </aside>
                <main class="pim-main git-main" tabindex="-1" aria-label=move || title.get()>
                    {move || {
                        current_repo.get().is_none().then(|| view! {
                            <div class="git-heading-bar">
                                <h1>{move || title.get()}</h1>
                                {move || {
                                    if view_mode.get() == View::Repositories {
                                        repos.with(|r| r.as_ref().map(|items| view! {
                                            <span class="git-heading-count">{items.len()}</span>
                                        }))
                                    } else {
                                        None
                                    }
                                }}
                            </div>
                        })
                    }}
                    {move || {
                        let err = error.get();
                        (!err.is_empty() && repos.with(Option::is_some)).then(|| view! {
                            <div role="alert" class="error git-error">
                                {err}
                                <button
                                    class="text-button"
                                    on:click=move |_| {
                                        error.set(String::new());
                                        reload.update(|n| *n += 1);
                                    }
                                >
                                    "Reload"
                                </button>
                            </div>
                        })
                    }}
                    {move || {
                        let err = error.get();
                        let list = repos.get();
                        if !err.is_empty() && list.is_none() {
                            return view! {
                                <div class="git-empty" role="alert">
                                    <h2>"Git could not be loaded"</h2>
                                    <p>{err}</p>
                                    <button class="secondary-button" on:click=move |_| reload.update(|n| *n += 1)>"Try again"</button>
                                </div>
                            }.into_any();
                        }
                        let Some(list) = list else {
                            return view! { <p class="git-empty" role="status">"Loading repositories\u{2026}"</p> }.into_any();
                        };
                        if let Some(id) = repo_id.get() {
                            let Some(repo) = list.iter().find(|r| r.id == id).cloned() else {
                                return view! {
                                    <div class="git-empty" role="alert">
                                        <h2>"This repository is not available"</h2>
                                        <p>"The folder may have stopped keeping history, or you no longer have access."</p>
                                        <button class="secondary-button" on:click=move |_| choose.run(View::Repositories)>"Show repositories"</button>
                                    </div>
                                }.into_any();
                            };
                            let repo_sig = Signal::derive(move || {
                                repos
                                    .with(|items| items.as_ref().and_then(|l| l.iter().find(|r| r.id == id).cloned()))
                                    .unwrap_or_else(|| repo.clone())
                            });
                            return view! {
                                <Repository
                                    repo=repo_sig
                                    session=session_sv.get_value()
                                    tab=tab
                                    on_tab=Callback::new(move |next| {
                                        tab.set(next);
                                        sheet.set(None);
                                    })
                                    tracker_version=tracker_version
                                    on_edit=Callback::new(move |e| editing.set(Some(e)))
                                    git_ref=git_ref
                                    on_ref=Callback::new(move |r| git_ref.set(r))
                                    path=path
                                    on_path=Callback::new(move |p| path.set(p))
                                    sheet=sheet
                                    on_sheet=Callback::new(move |s| sheet.set(s))
                                    notify=notify
                                    on_changed=Callback::new(move |_| reload.update(|n| *n += 1))
                                />
                            }.into_any();
                        }
                        if list.is_empty() {
                            return view! {
                                <div class="git-empty">
                                    <span class="empty-icon"><Icon name="repo"/></span>
                                    <h2>"No repositories yet"</h2>
                                    <p>"A folder becomes a repository when it keeps history. Start a new one, or keep history on a folder you already have."</p>
                                    <div class="git-empty-actions">
                                        <button class="primary-button" on:click=move |_| creating.set(true)><Icon name="plus"/>"New repository"</button>
                                        <a class="secondary-button" href="/">"Open Files"</a>
                                    </div>
                                </div>
                            }.into_any();
                        }
                        match view_mode.get() {
                            View::Overview => view! {
                                <Overview repos=list on_open=Callback::new(move |id| open_repo.run((id, Tab::Code)))/>
                            }.into_any(),
                            View::Repositories => view! {
                                <RepositoryList repos=list on_open=Callback::new(move |id| open_repo.run((id, Tab::Code)))/>
                            }.into_any(),
                            View::Agent => view! {
                                <AgentBranches repos=list on_open=Callback::new(move |id| open_repo.run((id, Tab::Branches)))/>
                            }.into_any(),
                        }
                    }}
                </main>
                {move || {
                    let current = sheet.get()?;
                    let repo = if matches!(current, Sheet::Issue { .. }) {
                        current_repo_id.get()?;
                        current_repo.get_untracked()?
                    } else {
                        current_repo.get()?
                    };
                    let label = match &current {
                        Sheet::Commit { .. } => "Commit",
                        Sheet::Compare { .. } => "Compare",
                        Sheet::Issue { .. } => "Conversation",
                        Sheet::File { .. } => "File",
                    };
                    let body = match current {
                        Sheet::Issue { number } => view! {
                            <IssueSheet
                                repo=repo
                                number=number
                                session=session_sv.get_value()
                                version=issue_version
                                notify=notify
                                on_close=Callback::new(move |_| sheet.set(None))
                                on_open=Callback::new(move |num| sheet.set(Some(Sheet::Issue { number: num })))
                                on_commit=Callback::new(move |sha| sheet.set(Some(Sheet::Commit { sha })))
                                on_edit=Callback::new(move |e| editing.set(Some(e)))
                                on_changed=tracker_changed
                            />
                        }.into_any(),
                        Sheet::Commit { sha } => view! {
                            <CommitSheet
                                repo=repo
                                sha=sha
                                session=session_sv.get_value()
                                on_changed=Callback::new(move |_| reload.update(|n| *n += 1))
                                on_close=Callback::new(move |_| sheet.set(None))
                                on_browse=Callback::new(move |sha| {
                                    tab.set(Tab::Code);
                                    git_ref.set(sha);
                                    path.set(String::new());
                                    sheet.set(None);
                                })
                                on_commit=Callback::new(move |sha| sheet.set(Some(Sheet::Commit { sha })))
                                notify=notify
                            />
                        }.into_any(),
                        Sheet::Compare { base, head } => view! {
                            <CompareSheet
                                repo=repo
                                session=session_sv.get_value()
                                on_pull=Callback::new(move |(b, h)| {
                                    editing.set(Some(IssueEditing::Issue {
                                        kind: "pull".to_owned(),
                                        issue: None,
                                        base: Some(b),
                                        head: Some(h),
                                    }));
                                })
                                notify=notify
                                on_merged=Callback::new(move |_| {
                                    sheet.set(None);
                                    reload.update(|n| *n += 1);
                                })
                                base=base
                                head=head
                                on_change=Callback::new(move |(b, h)| sheet.set(Some(Sheet::Compare { base: b, head: h })))
                                on_commit=Callback::new(move |sha| sheet.set(Some(Sheet::Commit { sha })))
                                on_close=Callback::new(move |_| sheet.set(None))
                            />
                        }.into_any(),
                        Sheet::File { git_ref: fref, path: fpath } => view! {
                            <FileSheet
                                repo=repo
                                git_ref=fref
                                path=fpath
                                on_close=Callback::new(move |_| sheet.set(None))
                            />
                        }.into_any(),
                    };
                    Some(view! {
                        <aside class="pim-details git-details" aria-label=label>
                            {body}
                        </aside>
                    })
                }}
            </div>
            {move || {
                current_repo_id.get()?;
                let repo = current_repo.get_untracked()?;
                let edit = editing.get()?;
                match edit {
                    IssueEditing::Issue { kind, issue, base, head } => {
                        let is_pull = kind == "pull";
                        let is_existing = issue.is_some();
                        Some(view! {
                            <IssueEditor
                                repo=repo
                                kind=kind
                                existing=issue
                                initial_head=head
                                initial_base=base
                                session=session_sv.get_value()
                                on_close=Callback::new(move |_| editing.set(None))
                                on_saved=Callback::new(move |number| {
                                    editing.set(None);
                                    tracker_changed.run(());
                                    tab.set(if is_pull { Tab::Pulls } else { Tab::Issues });
                                    sheet.set(Some(Sheet::Issue { number }));
                                    notify.run(if is_existing {
                                        "Saved".to_owned()
                                    } else {
                                        format!("Opened #{number}")
                                    });
                                })
                            />
                        }.into_any())
                    }
                    IssueEditing::Labels => Some(view! {
                        <LabelsEditor
                            repo=repo
                            session=session_sv.get_value()
                            on_close=Callback::new(move |_| editing.set(None))
                            on_changed=tracker_changed
                        />
                    }.into_any()),
                }
            }}
            {move || creating.get().then(|| view! {
                <NewRepository
                    session=session_sv.get_value()
                    on_close=Callback::new(move |_| creating.set(false))
                    on_created=Callback::new(move |id: String| {
                        creating.set(false);
                        reload.update(|n| *n += 1);
                        open_repo.run((id, Tab::Code));
                        notify.run("Repository ready".to_owned());
                    })
                />
            })}
            <div class="git-toast-region" role="status" aria-live="polite">
                {move || notice.get().map(|n| view! {
                    <div class="settings-toast git-toast"><span>{n.message}</span></div>
                })}
            </div>
        </div>
    }
}

#[component]
fn RepoMark(#[prop(into)] name: String, #[prop(optional, into)] size: &'static str) -> impl IntoView {
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_owned());
    let class = format!("git-mark hue-{} {size}", hue(&name));
    view! { <span class=class aria-hidden="true">{initial}</span> }
}

/// Commits per week as bars; heights are fixed classes scaled to the busiest week.
#[component]
fn Spark(weeks: Vec<usize>, #[prop(into)] label: String) -> impl IntoView {
    let max = weeks.iter().copied().max().unwrap_or(1).max(1) as f64;
    view! {
        <span class="git-spark" role="img" aria-label=label>
            {weeks.into_iter().map(|value| {
                let h = if value == 0 {
                    0
                } else {
                    ((value as f64 / max * 8.0).round() as usize).max(1)
                };
                view! { <i class=format!("h{h}")></i> }
            }).collect_view()}
        </span>
    }
}

#[component]
fn Refs(refs: Vec<GitRef>) -> impl IntoView {
    view! {
        {refs.into_iter().map(|item| {
            let icon = if item.kind == "tag" { "tag" } else { "branch" };
            view! {
                <span class=format!("git-ref is-{}", item.kind)>
                    <Icon name=icon/>
                    {item.name}
                </span>
            }
        }).collect_view()}
    }
}

// ---- Overview, list and Agent branches ---------------------------------------------------------

#[component]
fn Overview(repos: Vec<GitRepository>, on_open: Callback<String>) -> impl IntoView {
    let activity = use_load::<GitActivity>(Signal::derive(move || Some(format!("{API}/activity"))), Signal::derive(|| 0u64));
    let repo_count = repos.len();
    view! {
        <div class="git-scroll git-overview">
            {move || {
                let err = activity.error.get();
                if !err.is_empty() {
                    view! { <Failure error=activity.error retry=activity.retry what="Activity"/> }.into_any()
                } else if let Some(act) = activity.value.get() {
                    view! { <Heatmap times=act.times repos=repo_count/> }.into_any()
                } else {
                    view! { <Loading label="Loading activity"/> }.into_any()
                }
            }}
            <div class="git-overview-grid">
                <section aria-label="Repositories">
                    <h2 class="git-section-title">"Repositories"</h2>
                    <div class="git-cards">
                        {repos.into_iter().map(|repo| {
                            let id = repo.id.clone();
                            let desc_class = if repo.description.is_empty() { "git-description is-empty" } else { "git-description" };
                            let desc = repo.error.clone().unwrap_or_else(|| {
                                if repo.description.is_empty() { "No description".to_owned() } else { repo.description.clone() }
                            });
                            let spark_label = format!("Commits per week for {}", repo.name);
                            view! {
                                <button type="button" class="git-card" on:click=move |_| { on_open.try_run(id.clone()); }>
                                    <span class="git-card-head">
                                        <RepoMark name=repo.name.clone() size="is-m"/>
                                        <strong>{repo.name}</strong>
                                        {(repo.dirty > 0).then(|| view! {
                                            <span class="git-dirty-dot"><span class="sr-only">"Uncommitted changes in Dar"</span></span>
                                        })}
                                        <small>{edited(repo.updated_at)}</small>
                                    </span>
                                    <span class=desc_class>{desc}</span>
                                    <span class="git-card-meta">
                                        <span><Icon name="commit"/>{repo.commits}</span>
                                        <span><Icon name="branch"/>{repo.branches}</span>
                                        <span><Icon name="tag"/>{repo.tags}</span>
                                        <Spark weeks=repo.weeks label=spark_label/>
                                    </span>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </section>
                <section aria-label="Recent activity">
                    <h2 class="git-section-title">"Recent activity"</h2>
                    {move || {
                        if !activity.error.with(String::is_empty) {
                            return ().into_any();
                        }
                        let Some(act) = activity.value.get() else {
                            return view! { <Loading label="Loading recent activity"/> }.into_any();
                        };
                        if act.events.is_empty() {
                            return view! { <p class="git-hint">"Commits from Git, Dar and Agent appear here."</p> }.into_any();
                        }
                        view! {
                            <ol class="git-feed">
                                {act.events.into_iter().map(|event| {
                                    let repo_id = event.repo_id.clone();
                                    view! {
                                        <li>
                                            <Person name=event.author.clone() source=event.source.clone()/>
                                            <div>
                                                <span class="git-feed-what">
                                                    <strong>{event.author}</strong>
                                                    " committed to "
                                                    <button type="button" class="git-link" on:click=move |_| { on_open.try_run(repo_id.clone()); }>{event.repo_name}</button>
                                                    <Refs refs=event.refs/>
                                                </span>
                                                <span class="git-feed-message">{event.subject}</span>
                                                <span class="git-feed-meta">
                                                    <SourceChip source=event.source/>
                                                    {edited(event.time)}
                                                </span>
                                            </div>
                                        </li>
                                    }
                                }).collect_view()}
                            </ol>
                        }.into_any()
                    }}
                </section>
            </div>
        </div>
    }
}

struct HeatCell {
    time_ms: f64,
    count: usize,
    level: u8,
    future: bool,
}

/// A year of commits per local day, Sunday-first columns ending this week.
#[component]
fn Heatmap(times: Vec<f64>, repos: usize) -> impl IntoView {
    let today = Date::new_0();
    today.set_hours(0);
    today.set_minutes(0);
    today.set_seconds(0);
    today.set_milliseconds(0);
    let today_ms = today.get_time();

    let first_year = today.get_full_year();
    let first_month = today.get_month() as i32;
    let first_day_offset = today.get_date() as i32 - 364 - today.get_day() as i32;
    let first = Date::new_with_year_month_day(first_year, first_month, first_day_offset);
    let first_ms = first.get_time();

    let mut counts: HashMap<i64, usize> = HashMap::new();
    for t in times {
        let day = Date::new(&JsValue::from_f64(t * 1000.0));
        day.set_hours(0);
        day.set_minutes(0);
        day.set_seconds(0);
        day.set_milliseconds(0);
        let ms = day.get_time();
        if ms >= first_ms && ms <= today_ms {
            *counts.entry(ms as i64).or_insert(0) += 1;
        }
    }

    let mut values: Vec<usize> = counts.values().copied().collect();
    values.sort_unstable();
    let at = |q: f64| -> usize {
        if values.is_empty() {
            1
        } else {
            let idx = ((values.len() as f64) * q).floor() as usize;
            values[idx.min(values.len() - 1)]
        }
    };
    let q25 = at(0.25);
    let q50 = at(0.5);
    let q80 = at(0.8);
    let level_of = |count: usize| -> u8 {
        if count == 0 {
            0
        } else if count <= q25 {
            1
        } else if count <= q50 {
            2
        } else if count <= q80 {
            3
        } else {
            4
        }
    };

    let mut cells: Vec<HeatCell> = Vec::with_capacity(53 * 7);
    let mut months: Vec<String> = Vec::with_capacity(53);
    for step in 0..(53 * 7) {
        let day = Date::new_with_year_month_day(first_year, first_month, first_day_offset + step as i32);
        let ms = day.get_time();
        let count = counts.get(&(ms as i64)).copied().unwrap_or(0);
        cells.push(HeatCell {
            time_ms: ms,
            count,
            level: level_of(count),
            future: ms > today_ms,
        });
        if day.get_day() == 0 {
            months.push(if day.get_date() <= 7 {
                local_date(ms, &[("month", "short")])
            } else {
                String::new()
            });
        }
    }
    let total: usize = counts.values().sum();

    // Recent weeks matter most: a narrow screen starts scrolled to today.
    let scroller = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        let Some(node) = scroller.get() else { return };
        let scroll_to_end = {
            let node = node.clone();
            move || {
                node.set_scroll_left(node.scroll_width());
            }
        };
        scroll_to_end();
        let cb = Closure::<dyn FnMut()>::new(scroll_to_end);
        if let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
            observer.observe(&node);
            let observer = send_wrapper::SendWrapper::new(observer);
            let cb = send_wrapper::SendWrapper::new(cb);
            on_cleanup(move || {
                observer.disconnect();
                drop(cb);
            });
        }
    });

    view! {
        <section class="git-heat-card" aria-label="Activity over the last year">
            <div class="git-heat-head">
                <strong>{format!("{} in the last year", plural(total, "commit", None))}</strong>
                <span>{format!("across {}", plural(repos, "repository", Some("repositories")))}</span>
                <span class="git-heat-legend" aria-hidden="true">
                    "Less"
                    <i class="l0"></i>
                    <i class="l1"></i>
                    <i class="l2"></i>
                    <i class="l3"></i>
                    <i class="l4"></i>
                    "More"
                </span>
            </div>
            <div node_ref=scroller class="git-heat-scroll">
                <div class="git-heat-months" aria-hidden="true">
                    {months.into_iter().map(|m| view! { <span>{m}</span> }).collect_view()}
                </div>
                <div class="git-heat-body">
                    <div class="git-heat-days" aria-hidden="true">
                        <span></span><span>"Mon"</span><span></span><span>"Wed"</span><span></span><span>"Fri"</span><span></span>
                    </div>
                    <div class="git-heat" role="img" aria-label=format!("{} over the last year", plural(total, "commit", None))>
                        {cells.into_iter().map(|cell| {
                            let cls = if cell.future { "is-future".to_owned() } else { format!("l{}", cell.level) };
                            let title = (!cell.future).then(|| {
                                format!(
                                    "{} on {}",
                                    plural(cell.count, "commit", None),
                                    local_date(cell.time_ms, &[("dateStyle", "medium")]),
                                )
                            });
                            view! { <i class=cls title=title></i> }
                        }).collect_view()}
                    </div>
                </div>
            </div>
        </section>
    }
}

#[component]
fn RepositoryList(repos: Vec<GitRepository>, on_open: Callback<String>) -> impl IntoView {
    view! {
        <ul class="git-scroll git-repo-list" aria-label="Repositories">
            {repos.into_iter().map(|repo| {
                let id = repo.id.clone();
                let desc_class = if repo.description.is_empty() { "git-description is-empty" } else { "git-description" };
                let desc = repo.error.clone().unwrap_or_else(|| {
                    if repo.description.is_empty() {
                        "No description. Add a README.md to describe this repository.".to_owned()
                    } else {
                        repo.description.clone()
                    }
                });
                let spark_label = format!("Commits per week over 12 weeks for {}", repo.name);
                view! {
                    <li>
                        <button type="button" class="git-repo-row" on:click=move |_| { on_open.try_run(id.clone()); }>
                            <RepoMark name=repo.name.clone() size="is-l"/>
                            <span class="git-repo-body">
                                <span class="git-repo-name">
                                    <strong>{repo.name}</strong>
                                    <small>{repo.path}</small>
                                    {(repo.dirty > 0).then(|| view! {
                                        <span class="git-warn-chip"><Icon name="pencil"/>{format!("{} uncommitted", repo.dirty)}</span>
                                    })}
                                    {(repo.agent_branches > 0).then(|| view! {
                                        <span class="git-source is-agent"><Icon name="agent"/>{plural(repo.agent_branches, "Agent branch", Some("Agent branches"))}</span>
                                    })}
                                </span>
                                <span class=desc_class>{desc}</span>
                                <span class="git-repo-meta">
                                    <span><Icon name="commit"/>{plural(repo.commits, "commit", None)}</span>
                                    <span><Icon name="branch"/>{plural(repo.branches, "branch", Some("branches"))}</span>
                                    <span><Icon name="tag"/>{plural(repo.tags, "tag", None)}</span>
                                    <span><Icon name="clock"/>{format!("Updated {}", edited(repo.updated_at))}</span>
                                    <span>{format_bytes(repo.size as f64)}</span>
                                </span>
                            </span>
                            <Spark weeks=repo.weeks label=spark_label/>
                        </button>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
}

#[component]
fn AgentBranches(repos: Vec<GitRepository>, on_open: Callback<String>) -> impl IntoView {
    let with_branches: Vec<GitRepository> = repos.into_iter().filter(|r| r.agent_branches > 0).collect();
    let with_branches_sv = StoredValue::new(with_branches);
    let rows: RwSignal<Option<Vec<(GitRepository, GitAgentBranch)>>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let attempt = RwSignal::new(0u64);

    Effect::new(move |_| {
        let _ = attempt.get();
        error.set(String::new());
        let list = with_branches_sv.get_value();
        let abort = web_sys::AbortController::new().ok();
        let signal = abort.as_ref().map(|a| a.signal());
        if let Some(abort) = abort {
            let abort = send_wrapper::SendWrapper::new(abort);
            on_cleanup(move || abort.abort());
        }
        spawn_local(async move {
            let check = signal.clone();
            let mut all: Vec<(GitRepository, GitAgentBranch)> = Vec::new();
            for repo in list {
                let res = api::request::<GitBranches>(
                    &format!("{}/branches", repo_api(&repo.id)),
                    Req { signal: signal.clone(), ..Default::default() },
                )
                .await;
                match res {
                    Ok(value) => {
                        for branch in value.agent {
                            all.push((repo.clone(), branch));
                        }
                    }
                    Err(cause) => {
                        if !check.as_ref().is_some_and(|s| s.aborted()) {
                            let _ = error.try_set(cause.message);
                        }
                        return;
                    }
                }
            }
            all.sort_by(|a, b| {
                b.1.modified_at
                    .partial_cmp(&a.1.modified_at)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if !check.as_ref().is_some_and(|s| s.aborted()) {
                let _ = rows.try_set(Some(all));
            }
        });
    });

    view! {
        {move || {
            if !error.with(String::is_empty) {
                return view! {
                    <div class="git-scroll">
                        <Failure error=error retry=Callback::new(move |_| attempt.update(|n| *n += 1)) what="Agent branches"/>
                    </div>
                }.into_any();
            }
            let Some(list) = rows.get() else {
                return view! { <div class="git-scroll"><Loading label="Loading Agent branches"/></div> }.into_any();
            };
            if list.is_empty() {
                return view! {
                    <div class="git-empty">
                        <span class="empty-icon"><Icon name="agent"/></span>
                        <h2>"No Agent branches"</h2>
                        <p>"When Agent works on a folder that keeps history, its changes wait on a branch here until you merge them in Agent."</p>
                    </div>
                }.into_any();
            }
            view! {
                <ul class="git-scroll git-branch-list" aria-label="Agent branches">
                    {list.into_iter().map(|(repo, branch)| {
                        let repo_id = repo.id.clone();
                        let msg = if branch.message.is_empty() { "No message".to_owned() } else { branch.message };
                        let href = format!("/agent?thread={}", encode_uri(&branch.thread_id));
                        view! {
                            <li class="git-branch-row">
                                <Icon name="branch" class="is-agent"/>
                                <div class="git-branch-body">
                                    <span class="git-branch-name">{branch.name}</span>
                                    <span>
                                        {format!("{msg} \u{00b7} ")}
                                        <button type="button" class="git-link" on:click=move |_| { on_open.try_run(repo_id.clone()); }>{repo.name}</button>
                                        {format!(" \u{00b7} {} \u{00b7} {}", plural(branch.files, "file", None), edited(branch.modified_at))}
                                    </span>
                                </div>
                                <a class="secondary-button" href=href><Icon name="external"/>"Open in Agent"</a>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            }.into_any()
        }}
    }
}

// ---- A repository ------------------------------------------------------------------------------

#[component]
fn Repository(
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
    notify: Callback<String>,
    on_changed: Callback<()>,
    tracker_version: RwSignal<u64>,
    on_edit: Callback<IssueEditing>,
) -> impl IntoView {
    let version = RwSignal::new(0u64);
    let refs: RwSignal<Option<RefLists>> = RwSignal::new(None);
    let refs_error = RwSignal::new(String::new());
    let confirm: RwSignal<Option<DeleteTarget>> = RwSignal::new(None);
    let tab_ids = unique_id();
    let session_sv = StoredValue::new(session.clone());

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
                Req { signal: signal.clone(), ..Default::default() },
            );
            let tags_fut = api::request::<Vec<GitTag>>(
                &tags_url,
                Req { signal, ..Default::default() },
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
        { on_changed.try_run(()); };
    });

    let pull_from = Callback::new(move |head: String| {
        { on_edit.try_run(IssueEditing::Issue {
            kind: "pull".to_owned(),
            issue: None,
            base: Some("main".to_owned()),
            head: Some(head),
        }); };
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
                            on:click=move |_| { on_tab.try_run(t); }
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
                    let current_tab = tab.get();
                    let r = repo.get();
                    let write = r.role != "read";
                    match current_tab {
                        Tab::Issues | Tab::Pulls => view! {
                            <IssuesTab
                                repo=r
                                kind=if current_tab == Tab::Pulls { "pull" } else { "issue" }
                                version=tracker_version
                                selected=Signal::derive(move || match sheet.get() {
                                    Some(Sheet::Issue { number }) => Some(number),
                                    _ => None,
                                })
                                on_open=Callback::new(move |number| { on_sheet.try_run(Some(Sheet::Issue { number })); })
                                on_edit=on_edit
                            />
                        }.into_any(),
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
                                    { on_sheet.try_run(Some(Sheet::File {
                                        git_ref: if current.is_empty() { "main".to_owned() } else { current },
                                        path: file,
                                    })); };
                                })
                                on_commits=Callback::new(move |_| { on_tab.try_run(Tab::Commits); })
                                on_commit=Callback::new(move |sha| { on_sheet.try_run(Some(Sheet::Commit { sha })); })
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
                                on_commit=Callback::new(move |sha| { on_sheet.try_run(Some(Sheet::Commit { sha })); })
                            />
                        }.into_any(),
                        Tab::Branches => view! {
                            <BranchesTab
                                repo=r
                                refs=refs
                                write=write
                                on_pull=pull_from
                                on_compare=Callback::new(move |head| { on_sheet.try_run(Some(Sheet::Compare { base: "main".to_owned(), head })); })
                                on_commit=Callback::new(move |sha| { on_sheet.try_run(Some(Sheet::Commit { sha })); })
                                on_delete=Callback::new(move |branch: GitBranch| {
                                    confirm.set(Some(DeleteTarget {
                                        name: format!("refs/heads/{}", branch.name),
                                        old: branch.sha,
                                        kind: "branch",
                                        commits: Some(branch.ahead),
                                    }));
                                })
                                on_code=Callback::new(move |name| {
                                    { on_ref.try_run(name); };
                                    { on_path.try_run(String::new()); };
                                    { on_tab.try_run(Tab::Code); };
                                })
                                notify=notify
                            />
                        }.into_any(),
                        Tab::Tags => view! {
                            <TagsTab
                                repo=r
                                refs=refs
                                write=write
                                on_commit=Callback::new(move |sha| { on_sheet.try_run(Some(Sheet::Commit { sha })); })
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
                        }.into_any(),
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
                                    { on_ref.try_run(String::new()); };
                                }
                                if matches!(sheet.get_untracked(), Some(Sheet::Compare { .. })) {
                                    { on_sheet.try_run(None); };
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
            plural(target.commits.unwrap_or(0), "commit is", Some("commits are"))
        )
    } else {
        format!("{name} is removed from {}. Its commits are also on main, so nothing else changes.", repo.name)
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
            <button class="text-button" on:click=move |_| { on_cancel.try_run(()); }>"Cancel"</button>
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
                            Ok(_) => { on_deleted.try_run(()); },
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

/// Clone: the alias and the permanent address, downloads and a way to an app password.
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
            let target = event.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            let in_panel = panel.get_untracked().is_some_and(|p| p.contains(target.as_ref()));
            let in_trigger = trigger.get_untracked().is_some_and(|t| t.contains(target.as_ref()));
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

#[component]
fn RefPicker(
    refs: RwSignal<Option<RefLists>>,
    value: Signal<String>,
    on_change: Callback<String>,
    #[prop(optional)] all: bool,
    #[prop(into)] label: String,
) -> impl IntoView {
    let options = Signal::derive(move || {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut branches: Vec<String> = Vec::new();
        let mut tags: Vec<String> = Vec::new();
        if let Some(lists) = refs.get() {
            if lists.branches.main.is_some() {
                branches.push("main".to_owned());
            }
            for b in lists.branches.branches {
                branches.push(b.name);
            }
            for t in lists.tags {
                tags.push(t.name);
            }
        } else {
            branches.push("main".to_owned());
        }
        let current = value.get();
        let known = current.is_empty()
            || current == "all"
            || branches.iter().any(|b| b == &current)
            || tags.iter().any(|t| t == &current);
        if all {
            out.push(("all".to_owned(), "All branches".to_owned()));
        }
        for name in branches {
            out.push((name.clone(), name));
        }
        for name in tags {
            out.push((name.clone(), format!("{name} (tag)")));
        }
        if !known {
            out.push((current.clone(), short(&current)));
        }
        out
    });

    let display_value = Signal::derive(move || {
        let v = value.get();
        if v.is_empty() {
            if all { "all".to_owned() } else { "main".to_owned() }
        } else {
            v
        }
    });

    view! {
        <GitSelect
            aria_label=label
            class="git-ref-picker"
            value=display_value
            options=options
            on_change=Callback::new(move |next: String| {
                if next == "all" || (next == "main" && !all) {
                    { on_change.try_run(String::new()); };
                } else {
                    { on_change.try_run(next); };
                }
            })
        />
    }
}

#[component]
fn Code(
    repo: Signal<GitRepository>,
    refs: RwSignal<Option<RefLists>>,
    git_ref: RwSignal<String>,
    on_ref: Callback<String>,
    path: RwSignal<String>,
    on_path: Callback<String>,
    on_file: Callback<String>,
    on_commits: Callback<()>,
    on_commit: Callback<String>,
) -> impl IntoView {
    let tree_url = Signal::derive(move || {
        let r = repo.get();
        let _rev = r.revision;
        let current_ref = git_ref.get();
        let current_path = path.get();
        let mut params: Vec<(&str, String)> = Vec::new();
        if !current_ref.is_empty() {
            params.push(("ref", current_ref));
        }
        if !current_path.is_empty() {
            params.push(("path", current_path));
        }
        let pairs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        Some(format!("{}/tree?{}", repo_api(&r.id), query_string(&pairs)))
    });
    let latest_url = Signal::derive(move || {
        let r = repo.get();
        let _rev = r.revision;
        let current_ref = git_ref.get();
        let ref_val = if current_ref.is_empty() { "main" } else { current_ref.as_str() };
        Some(format!(
            "{}/commits?{}",
            repo_api(&r.id),
            query_string(&[("ref", ref_val), ("limit", "1")]),
        ))
    });

    let tree = use_load::<GitTree>(tree_url, Signal::derive(move || repo.with(|r| r.revision as u64)));
    let latest = use_load::<GitCommitPage>(latest_url, Signal::derive(move || repo.with(|r| r.revision as u64)));

    view! {
        {move || {
            let r = repo.get();
            if r.commits == 0 && r.branches == 0 {
                return view! {
                    <div class="git-empty">
                        <span class="empty-icon"><Icon name="commit"/></span>
                        <h2>"No commits yet"</h2>
                        <p>"Commit the folder in Files, or push to it with Git:"</p>
                        <pre class="git-command">{format!("git clone {}", r.clone.alias)}</pre>
                    </div>
                }.into_any();
            }
            let current_path = path.get();
            let current_ref = git_ref.get();
            let parts: Vec<String> = if current_path.is_empty() {
                Vec::new()
            } else {
                current_path.split('/').map(str::to_owned).collect()
            };
            let parts_len = parts.len();
            let parent_path = if parts_len > 1 {
                parts[..parts_len - 1].join("/")
            } else {
                String::new()
            };
            let caption = if current_path.is_empty() {
                format!("Files in {}", r.name)
            } else {
                format!("Files in {current_path}")
            };

            view! {
                <div class="git-scroll git-code">
                    <div class="git-toolbar">
                        <RefPicker
                            refs=refs
                            value=git_ref.into()
                            on_change=Callback::new(move |val| {
                                { on_ref.try_run(val); };
                                { on_path.try_run(String::new()); };
                            })
                            label="Branch or tag"
                        />
                        <nav class="git-path" aria-label="Folder">
                            <button type="button" class="git-link" on:click=move |_| { on_path.try_run(String::new()); }>{r.name.clone()}</button>
                            {parts.iter().enumerate().map(|(index, part)| {
                                let is_last = index == parts_len - 1;
                                let part_text = part.clone();
                                let crumb_path = parts[..=index].join("/");
                                view! {
                                    <span>
                                        <Icon name="chevron"/>
                                        {if is_last {
                                            view! { <strong aria-current="page">{part_text}</strong> }.into_any()
                                        } else {
                                            view! { <button type="button" class="git-link" on:click=move |_| { on_path.try_run(crumb_path.clone()); }>{part_text}</button> }.into_any()
                                        }}
                                    </span>
                                }
                            }).collect_view()}
                        </nav>
                    </div>
                    {(r.dirty > 0 && current_ref.is_empty()).then(|| {
                        let files_href = format!("/?folder={}&file={}", r.parent_id, r.id);
                        let pronoun = if r.dirty == 1 { "it" } else { "them" };
                        let verb = if r.dirty == 1 { "it is" } else { "they are" };
                        view! {
                            <div class="git-banner">
                                <Icon name="pencil"/>
                                <div>
                                    <strong>{format!("{} changed in Dar since the last commit", plural(r.dirty as usize, "file", None))}</strong>
                                    <span>{format!("Pushes that change {pronoun} are refused until {verb} committed.")}</span>
                                </div>
                                <a class="text-button" href=files_href>"Review and commit"</a>
                            </div>
                        }
                    })}
                    {move || {
                        if !tree.error.with(String::is_empty) {
                            return view! { <Failure error=tree.error retry=tree.retry what="Files"/> }.into_any();
                        }
                        let Some(tree_data) = tree.value.get() else {
                            return view! { <Loading label="Loading files"/> }.into_any();
                        };
                        let latest_page = latest.value.get();
                        let caption_text = caption.clone();
                        let has_path = !current_path.is_empty();
                        let parent_for_up = parent_path.clone();
                        let is_empty_dir = tree_data.entries.is_empty();
                        view! {
                            {latest_page.and_then(|page| {
                                let head = page.commits.into_iter().next()?;
                                let sha = head.sha.clone();
                                let total = page.total;
                                Some(view! {
                                    <div class="git-latest">
                                        <Person name=head.author.clone() source=head.source.clone()/>
                                        <strong>{head.author}</strong>
                                        <button type="button" class="git-link git-latest-message" on:click=move |_| { on_commit.try_run(sha.clone()); }>{head.subject}</button>
                                        <Sha sha=head.sha/>
                                        <span class="git-muted">{edited(head.time)}</span>
                                        <button type="button" class="text-button" on:click=move |_| { on_commits.try_run(()); }>
                                            <Icon name="history"/>
                                            {plural(total, "commit", None)}
                                        </button>
                                    </div>
                                })
                            })}
                            <table class="git-files">
                                <caption class="sr-only">{caption_text}</caption>
                                <thead class="sr-only">
                                    <tr><th>"Name"</th><th>"Last commit"</th><th>"Changed"</th></tr>
                                </thead>
                                <tbody>
                                    {has_path.then(|| {
                                        let up = parent_for_up.clone();
                                        view! {
                                            <tr>
                                                <td colspan="3">
                                                    <button type="button" class="git-file-name" on:click=move |_| { on_path.try_run(up.clone()); }>
                                                        <Icon name="back"/>
                                                        ".."
                                                    </button>
                                                </td>
                                            </tr>
                                        }
                                    })}
                                    {tree_data.entries.into_iter().map(|entry| {
                                        let entry_path = entry.path.clone();
                                        let is_tree = entry.kind == "tree";
                                        let is_submodule = entry.kind == "submodule";
                                        let icon = match entry.kind.as_str() {
                                            "tree" => "folder",
                                            "link" => "link",
                                            _ => "file",
                                        };
                                        let cls = format!("git-file-name is-{}", entry.kind);
                                        let last_time = entry.last.as_ref().map(|l| edited(l.time)).unwrap_or_default();
                                        view! {
                                            <tr>
                                                <td>
                                                    <button
                                                        type="button"
                                                        class=cls
                                                        disabled=is_submodule
                                                        on:click=move |_| {
                                                            if is_tree {
                                                                { on_path.try_run(entry_path.clone()); };
                                                            } else {
                                                                { on_file.try_run(entry_path.clone()); };
                                                            }
                                                        }
                                                    >
                                                        <Icon name=icon/>
                                                        {entry.name}
                                                    </button>
                                                </td>
                                                <td class="git-file-commit">
                                                    {entry.last.map(|last| {
                                                        let sha = last.sha.clone();
                                                        view! {
                                                            <button type="button" class="git-link" on:click=move |_| { on_commit.try_run(sha.clone()); }>{last.subject}</button>
                                                        }
                                                    })}
                                                </td>
                                                <td class="git-file-time">{last_time}</td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                            {is_empty_dir.then(|| view! { <p class="git-hint">"This folder is empty."</p> })}
                            {tree_data.readme.map(|readme| {
                                let file_name = readme.path.rsplit('/').next().unwrap_or(&readme.path).to_owned();
                                let md = is_markdown_path(&readme.path);
                                view! {
                                    <section class="git-readme" aria-label=readme.path>
                                        <h2><Icon name="file"/>{file_name}</h2>
                                        <div class="git-readme-body">
                                            {if md {
                                                view! { <Markdown source=readme.text/> }.into_any()
                                            } else {
                                                view! { <pre>{readme.text}</pre> }.into_any()
                                            }}
                                        </div>
                                    </section>
                                }
                            })}
                        }.into_any()
                    }}
                </div>
            }.into_any()
        }}
    }
}

#[component]
fn Commits(
    repo: Signal<GitRepository>,
    refs: RwSignal<Option<RefLists>>,
    selected: Signal<Option<String>>,
    on_commit: Callback<String>,
) -> impl IntoView {
    let git_ref = RwSignal::new("all".to_owned());
    let author = RwSignal::new(String::new());
    let source = RwSignal::new(String::new());
    let initial_mode = window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item("dar.git.commits").ok().flatten())
        .map(|v| if v == "list" { "list".to_owned() } else { "graph".to_owned() })
        .unwrap_or_else(|| "graph".to_owned());
    let mode = RwSignal::new(initial_mode);
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
            let res = api::request::<GitCommitPage>(&url, Req { signal, ..Default::default() }).await;
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

    Effect::new(move |_| {
        let current = mode.get();
        if let Ok(Some(storage)) = window().local_storage() {
            let _ = storage.set_item("dar.git.commits", &current);
        }
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
                    value=git_ref.into()
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
                                            on_open=Callback::new(move |_| { on_commit.try_run(sha_click.clone()); })
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
                                            on_open=Callback::new(move |_| { on_commit.try_run(sha_click.clone()); })
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

const ROW_HEIGHT: f64 = 46.0;

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
            on:click=move |_| { on_open.try_run(()); }
        >
            {match graph {
                Some(row) => view! { <Lanes row=row/> }.into_any(),
                None => view! { <Person name=commit.author.clone() source=commit.source.clone()/> }.into_any(),
            }}
            <span class="git-commit-main">
                <span class="git-commit-subject">
                    {is_merge.then(|| view! { <Icon name="merge" class="git-merge-icon"/> })}
                    <span>{subject}</span>
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
fn BranchesTab(
    repo: GitRepository,
    refs: RwSignal<Option<RefLists>>,
    write: bool,
    on_pull: Callback<String>,
    on_compare: Callback<String>,
    on_commit: Callback<String>,
    on_delete: Callback<GitBranch>,
    on_code: Callback<String>,
    notify: Callback<String>,
) -> impl IntoView {
    let repo_id = repo.id.clone();

    view! {
        {move || {
            let Some(lists) = refs.get() else {
                return view! { <div class="git-scroll"><Loading label="Loading branches"/></div> }.into_any();
            };
            let GitBranches { main, branches, agent } = lists.branches;
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
                let menu_label = format!("Actions for {}", branch.name);
                let branch_for_menu = branch.clone();
                let actions = Callback::new(move |_| {
                    let b = branch_for_menu.clone();
                    let mut items: Vec<MenuAction> = Vec::new();
                    let code_name = if is_main { String::new() } else { b.name.clone() };
                    items.push(MenuAction::new("Browse files", "code").run(move || { on_code.try_run(code_name.clone()); }));
                    if !is_main {
                        let cmp = b.name.clone();
                        items.push(MenuAction::new("Compare with main", "compare").run(move || { on_compare.try_run(cmp.clone()); }));
                    }
                    if !is_main && write {
                        let pr = b.name.clone();
                        items.push(MenuAction::new("Open pull request", "pull").run(move || { on_pull.try_run(pr.clone()); }));
                    }
                    let mut zip = MenuAction::new("Download ZIP", "download").href(zip_url(&repo_id, &b.name));
                    zip.download = true;
                    items.push(zip);
                    let copy_name = b.name.clone();
                    items.push(MenuAction::new("Copy name", "copy").run(move || copy(copy_name.clone(), notify, "branch name")));
                    if !is_main && write {
                        let del_branch = b.clone();
                        let mut del = MenuAction::new("Delete branch", "trash").run(move || { on_delete.try_run(del_branch.clone()); });
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
                            </span>
                            <span>
                                <button type="button" class="git-link" on:click=move |_| { on_commit.try_run(sha.clone()); }>{branch.subject}</button>
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
                                <button type="button" class="secondary-button" on:click=move |_| { on_compare.try_run(compare_name.clone()); }>
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
                                    let msg = if branch.message.is_empty() { "No message".to_owned() } else { branch.message };
                                    let meta = format!("{} \u{00b7} {} \u{00b7} {}", msg, plural(branch.files, "file", None), edited(branch.modified_at));
                                    let href = format!("/agent?thread={}", encode_uri(&branch.thread_id));
                                    view! {
                                        <li class="git-branch-row">
                                            <Icon name="branch" class="is-agent"/>
                                            <div class="git-branch-body">
                                                <span class="git-branch-name">{branch.name}</span>
                                                <span>{meta}</span>
                                            </div>
                                            <span></span>
                                            <span class="git-branch-actions">
                                                {write.then(|| view! {
                                                    <button type="button" class="secondary-button" on:click=move |_| { on_pull.try_run(pr_name.clone()); }>
                                                        <Icon name="pull"/>
                                                        "Open pull request"
                                                    </button>
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
fn TagsTab(
    repo: GitRepository,
    refs: RwSignal<Option<RefLists>>,
    write: bool,
    on_commit: Callback<String>,
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
            view! {
                <ul class="git-scroll git-tag-list" aria-label="Tags">
                    {lists.tags.into_iter().map(|tag| {
                        let target_btn = tag.target.clone();
                        let target_sha = tag.target.clone();
                        let zip_href = zip_url(&repo_id, &tag.name);
                        let label = format!("Actions for {}", tag.name);
                        let msg = if tag.message.is_empty() { "Lightweight tag".to_owned() } else { tag.message.clone() };
                        let meta = format!("{} \u{00b7} {} \u{00b7} {}", msg, tag.author, edited(tag.time));
                        let tag_for_menu = tag.clone();
                        let actions = Callback::new(move |_| {
                            let t = tag_for_menu.clone();
                            let target_show = t.target.clone();
                            let copy_name = t.name.clone();
                            let mut items = vec![
                                MenuAction::new("Show commit", "commit").run(move || { on_commit.try_run(target_show.clone()); }),
                                MenuAction::new("Copy name", "copy").run(move || copy(copy_name.clone(), notify, "tag name")),
                            ];
                            if write {
                                let del_tag = t.clone();
                                let mut del = MenuAction::new("Delete tag", "trash").run(move || { on_delete.try_run(del_tag.clone()); });
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
                                    <span class="git-branch-name">{tag.name}</span>
                                    <span>{meta}</span>
                                </div>
                                <button type="button" class="git-link" on:click=move |_| { on_commit.try_run(target_btn.clone()); }>
                                    <Sha sha=target_sha/>
                                </button>
                                <span class="git-branch-actions">
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

// ---- Right sheet -------------------------------------------------------------------------------

#[component]
fn CommitSheet(
    repo: GitRepository,
    sha: String,
    session: BrowserSession,
    on_changed: Callback<()>,
    on_close: Callback<()>,
    on_browse: Callback<String>,
    on_commit: Callback<String>,
    notify: Callback<String>,
) -> impl IntoView {
    let url = format!("{}/commits/{sha}", repo_api(&repo.id));
    let load = use_load::<GitCommitDetail>(Signal::derive(move || Some(url.clone())), Signal::derive(|| 0u64));
    let wrap = RwSignal::new(false);
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
            <SheetHead title=title on_close=on_close>
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
                                                    <button type="button" class="git-link" on:click=move |_| { on_commit.try_run(p_click.clone()); }>
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
                            <label class="git-wrap">
                                <input
                                    type="checkbox"
                                    prop:checked=move || wrap.get()
                                    on:change=move |ev| wrap.set(event_target_checked(&ev))
                                />
                                "Wrap lines"
                            </label>
                        </div>
                        <FileChanges files=commit.files wrap=wrap/>
                    }.into_any()
                }}
            </div>
            <div class="git-sheet-foot">
                <button class="secondary-button" on:click=move |_| { on_browse.try_run(sha_browse.clone()); }>
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
                                { on_changed.try_run(()); };
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
    let restore = commit.restore.clone().expect("RestoreFolder requires restore metadata");
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
            <button class="text-button" on:click=move |_| { on_cancel.try_run(()); }>"Cancel"</button>
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
                            Ok(_) => { on_done.try_run(()); },
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

#[component]
fn CompareSheet(
    repo: GitRepository,
    session: BrowserSession,
    notify: Callback<String>,
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
    let compare = use_load::<GitCompare>(Signal::derive(move || Some(compare_url.clone())), Signal::derive(|| 0u64));
    let lists = use_load::<GitBranches>(Signal::derive(move || Some(branches_url.clone())), Signal::derive(|| 0u64));
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
        names.into_iter().map(|n| (n.clone(), n)).collect::<Vec<_>>()
    });

    view! {
        <div
            node_ref=root
            class="git-sheet"
            tabindex="-1"
            on:keydown=move |ev| sheet_keydown(ev, on_close)
        >
            <SheetHead title=view! { "Compare" }.into_any() on_close=on_close/>
            <div class="git-sheet-body">
                <div class="git-compare-pick">
                    <GitSelect
                        aria_label="Base branch"
                        value=Signal::derive(move || base_sv.get_value())
                        options=branch_options
                        on_change=Callback::new(move |next_base| { on_change.try_run((next_base, head_sv.get_value())); })
                    />
                    <Icon name="back"/>
                    <GitSelect
                        aria_label="Compared branch"
                        value=Signal::derive(move || head_sv.get_value())
                        options=branch_options
                        on_change=Callback::new(move |next_head| { on_change.try_run((base_sv.get_value(), next_head)); })
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
                                                <button type="button" on:click=move |_| { on_commit.try_run(sha.clone()); }>
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
                                </div>
                                <ul class="git-file-summary">
                                    {value.files.into_iter().map(|file| {
                                        let label = kind_label(&file.kind);
                                        let letter = label.chars().next().map(|c| c.to_string()).unwrap_or_default();
                                        let kind_cls = format!("git-kind-letter is-{}", file.kind);
                                        view! {
                                            <li>
                                                <span class=kind_cls title=label>
                                                    {letter}
                                                    <span class="sr-only">{label}</span>
                                                </span>
                                                <span class="git-diff-path">{file.path}</span>
                                                <span class="git-stat">
                                                    {if file.binary {
                                                        view! { "Binary" }.into_any()
                                                    } else {
                                                        view! {
                                                            <span class="git-added">{format!("+{}", file.added)}</span>
                                                            " "
                                                            <span class="git-removed">{format!("\u{2212}{}", file.removed)}</span>
                                                        }.into_any()
                                                    }}
                                                </span>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            }.into_any()
                        }}
                        {(has_ahead && can_write).then(|| view! {
                            <button
                                type="button"
                                class="secondary-button git-open-pull"
                                on:click=move |_| { on_pull.try_run((b_pull.clone(), h_pull.clone())); }
                            >
                                <Icon name="pull"/>
                                "Open pull request"
                            </button>
                            <MergeBox
                                repo=r
                                compare=compare_for_merge
                                session=session_sv.get_value()
                                on_merged=Callback::new(move |result: crate::git::parts::GitMergeResult| {
                                    notify.run(if result.fast_forward {
                                        format!("{b_merge} now points at {h_merge}")
                                    } else {
                                        format!("Merged {h_merge} into {b_merge}")
                                    });
                                    { on_merged.try_run(()); };
                                })
                            />
                        })}
                    }.into_any()
                }}
            </div>
        </div>
    }
}

#[derive(Clone)]
enum FileBody {
    Loading,
    Binary,
    Text(String),
}

#[component]
fn FileSheet(
    repo: GitRepository,
    git_ref: String,
    path: String,
    on_close: Callback<()>,
) -> impl IntoView {
    let url = format!(
        "{}/blob?{}",
        repo_api(&repo.id),
        query_string(&[("ref", git_ref.as_str()), ("path", path.as_str())])
    );
    let view_url = format!("{url}&view");
    let state = RwSignal::new(FileBody::Loading);
    let error = RwSignal::new(String::new());
    let root = NodeRef::<html::Div>::new();
    focus_sheet(root);

    Effect::new(move |_| {
        state.set(FileBody::Loading);
        error.set(String::new());
        let target = view_url.clone();
        let abort = web_sys::AbortController::new().ok();
        let signal = abort.as_ref().map(|a| a.signal());
        if let Some(abort) = abort {
            let abort = send_wrapper::SendWrapper::new(abort);
            on_cleanup(move || abort.abort());
        }
        spawn_local(async move {
            let init = web_sys::RequestInit::new();
            init.set_method("GET");
            if let Some(sig) = signal.as_ref() {
                init.set_signal(Some(sig));
            }
            let fetched = JsFuture::from(window().fetch_with_str_and_init(&target, &init)).await;
            if signal.as_ref().is_some_and(|s| s.aborted()) {
                return;
            }
            let response: web_sys::Response = match fetched.and_then(|v| v.dyn_into::<web_sys::Response>()) {
                Ok(r) => r,
                Err(_) => {
                    let _ = error.try_set("This file could not be loaded.".to_owned());
                    return;
                }
            };
            if !response.ok() {
                let _ = error.try_set("This file could not be loaded.".to_owned());
                return;
            }
            let content_type = response
                .headers()
                .get("content-type")
                .ok()
                .flatten()
                .unwrap_or_default();
            if content_type.starts_with("text/plain") {
                match response.text().map(JsFuture::from) {
                    Ok(fut) => match fut.await.ok().and_then(|v| v.as_string()) {
                        Some(text) => {
                            let _ = state.try_set(FileBody::Text(text));
                        }
                        None => {
                            let _ = error.try_set("This file could not be loaded.".to_owned());
                        }
                    },
                    Err(_) => {
                        let _ = error.try_set("This file could not be loaded.".to_owned());
                    }
                }
            } else {
                let _ = state.try_set(FileBody::Binary);
            }
        });
    });

    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
    let name_md = is_markdown_path(&name);
    let name_contents = format!("Contents of {name}");
    let download_label = format!("Download {name}");
    let ref_display = if git_ref.len() == 40 { short(&git_ref) } else { git_ref };
    let subtitle = format!("{path} \u{00b7} {ref_display}");

    view! {
        <div
            node_ref=root
            class="git-sheet"
            tabindex="-1"
            on:keydown=move |ev| sheet_keydown(ev, on_close)
        >
            <SheetHead title=view! { {name} }.into_any() on_close=on_close>
                <a class="icon-button" href=url download="" aria-label=download_label title="Download">
                    <Icon name="download"/>
                </a>
            </SheetHead>
            <div class="git-sheet-body">
                <p class="git-muted git-file-path">{subtitle}</p>
                {move || {
                    if !error.with(String::is_empty) {
                        return view! { <p class="error" role="alert">{error.get()}</p> }.into_any();
                    }
                    match state.get() {
                        FileBody::Loading => view! { <Loading label="Loading file"/> }.into_any(),
                        FileBody::Binary => view! { <p class="git-hint">"This file can\u{2019}t be shown here. Download it to open it."</p> }.into_any(),
                        FileBody::Text(text) => {
                            if name_md {
                                view! { <div class="git-readme-body"><Markdown source=text/></div> }.into_any()
                            } else {
                                view! { <pre class="git-source-view" tabindex="0" aria-label=name_contents.clone()>{text}</pre> }.into_any()
                            }
                        }
                    }
                }}
            </div>
        </div>
    }
}

// ---- New repository ----------------------------------------------------------------------------

#[derive(Deserialize)]
struct UploadReservation {
    id: String,
}

async fn track_folder(folder_id: &str, csrf: &str) -> Result<(), String> {
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    let headers = web_sys::Headers::new().map_err(|_| "History could not be turned on for this folder.".to_owned())?;
    let _ = headers.set("content-type", "application/json");
    let _ = headers.set("x-csrf-token", csrf);
    init.set_headers(&headers);
    let body = json!({ "action": "track" }).to_string();
    init.set_body(&JsValue::from_str(&body));

    let url = format!("/api/v1/files/history/{folder_id}");
    let resp: web_sys::Response = JsFuture::from(window().fetch_with_str_and_init(&url, &init))
        .await
        .and_then(|v| v.dyn_into::<web_sys::Response>())
        .map_err(|_| "History could not be turned on for this folder.".to_owned())?;
    if resp.ok() {
        return Ok(());
    }
    let text = match resp.text().map(JsFuture::from) {
        Ok(fut) => fut.await.ok().and_then(|v| v.as_string()).unwrap_or_default(),
        Err(_) => String::new(),
    };
    if text.contains("quotaExceeded") {
        Err("You keep history on as many folders as Dar allows.".to_owned())
    } else if text.contains("forbidden") {
        Err("History can\u{2019}t be kept here: the folder is in the Locked folder, inside another repository, or not yours.".to_owned())
    } else {
        Err("History could not be turned on for this folder.".to_owned())
    }
}

/// A new folder that keeps history (optionally starting with a README), or history on an existing folder.
#[component]
fn NewRepository(
    session: BrowserSession,
    on_close: Callback<()>,
    on_created: Callback<String>,
) -> impl IntoView {
    let mode = RwSignal::new("new".to_owned());
    let name = RwSignal::new(String::new());
    let readme = RwSignal::new(true);
    let place: RwSignal<Option<Selection>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let name_id = unique_id();
    let csrf = StoredValue::new(session.csrf_token);

    let valid = Memo::new(move |_| {
        if mode.get() == "existing" {
            place.with(|p| p.as_ref().is_some_and(|s| !s.id.is_empty()))
        } else {
            name.with(|n| is_valid_folder_name(n))
        }
    });

    let dirty = Signal::derive(move || !name.with(String::is_empty) && !busy.get());

    let mode_options = Signal::derive(move || {
        vec![
            ("new".to_owned(), "New folder".to_owned()),
            ("existing".to_owned(), "Existing folder".to_owned()),
        ]
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        if !valid.get_untracked() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        let current_mode = mode.get_untracked();
        let current_place = place.get_untracked();
        let title = name.get_untracked().trim().to_owned();
        let add_readme = readme.get_untracked();
        let token = csrf.get_value();

        spawn_local(async move {
            let result: Result<String, String> = async {
                if current_mode == "existing" {
                    let target_id = current_place
                        .map(|s| s.id)
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| "Choose a folder you own.".to_owned())?;
                    track_folder(&target_id, &token).await?;
                    return Ok(target_id);
                }

                let conn = live::connection()
                    .ok_or_else(|| "Git is reconnecting. Try again in a moment.".to_owned())?;
                let id = random_uuid();
                let parent_id = current_place.map(|s| s.id).unwrap_or_default();
                let (tx, rx) = futures::channel::oneshot::channel();
                conn.reducers
                    .create_folder_then(id.clone(), parent_id, title.clone(), live::replied(tx))
                    .map_err(|e| e.to_string())?;
                rx.await
                    .map_err(|_| "The connection closed while creating the folder.".to_owned())??;

                track_folder(&id, &token).await?;

                if add_readme {
                    let text = format!("# {title}\n");
                    let reservation = api::send::<UploadReservation>(
                        "/api/v1/files/uploads",
                        "POST",
                        Some(json!({
                            "name": "README.md",
                            "parentId": id,
                            "size": text.len(),
                            "mediaType": "text/markdown",
                        })),
                        &token,
                    )
                    .await
                    .map_err(|e| e.message)?;

                    let init = web_sys::RequestInit::new();
                    init.set_method("PUT");
                    let headers = web_sys::Headers::new()
                        .map_err(|_| "The README could not be added.".to_owned())?;
                    let _ = headers.set("x-csrf-token", &token);
                    init.set_headers(&headers);
                    init.set_body(&JsValue::from_str(&text));
                    let put_url = format!("/api/v1/files/uploads/{}", reservation.id);
                    let put_resp: web_sys::Response = JsFuture::from(
                        window().fetch_with_str_and_init(&put_url, &init),
                    )
                    .await
                    .and_then(|v| v.dyn_into::<web_sys::Response>())
                    .map_err(|_| "The README could not be added.".to_owned())?;
                    if !put_resp.ok() {
                        return Err("The README could not be added.".to_owned());
                    }

                    let status = api::get::<HistoryOverview>(&format!("/api/v1/files/history/{id}"))
                        .await
                        .map_err(|e| e.message)?;
                    let status_token = status
                        .status
                        .map(|s| s.token)
                        .ok_or_else(|| "The README could not be added.".to_owned())?;
                    api::send::<serde_json::Value>(
                        &format!("/api/v1/files/history/{id}/commits"),
                        "POST",
                        Some(json!({ "message": "Add README", "token": status_token })),
                        &token,
                    )
                    .await
                    .map_err(|e| e.message)?;
                }

                Ok(id)
            }
            .await;

            match result {
                Ok(id) => { on_created.try_run(id); },
                Err(msg) => {
                    let _ = error.try_set(msg);
                    let _ = busy.try_set(false);
                }
            }
        });
    };

    let name_id_sv = StoredValue::new(name_id);

    view! {
        <EditorPane title="New repository" on_close=on_close dirty=dirty>
            <form class="git-new" on:submit=submit>
                <GitSegmented
                    label="Start from"
                    value=mode
                    options=mode_options
                    on_change=Callback::new(move |val| mode.set(val))
                />
                {move || {
                    if mode.get() == "new" {
                        let nid = name_id_sv.get_value();
                        let nid_for = nid.clone();
                        view! {
                            <label class="git-field" for=nid_for>
                                "Name"
                                <input
                                    id=nid
                                    prop:value=move || name.get()
                                    maxlength="255"
                                    autofocus=true
                                    required=true
                                    aria-invalid=move || (!name.with(String::is_empty) && !valid.get()).to_string()
                                    on:input=move |ev| name.set(event_target_value(&ev))
                                />
                            </label>
                            <div class="git-field">
                                <span>"Location"</span>
                                <FolderPicker
                                    initial_folder_id=""
                                    exclude_ids=Vec::new()
                                    disabled=busy
                                    on_change=Callback::new(move |sel| place.set(sel))
                                />
                                <small>
                                    {move || {
                                        let where_name = place.with(|p| {
                                            p.as_ref()
                                                .filter(|s| !s.id.is_empty())
                                                .map(|s| s.name.clone())
                                                .unwrap_or_else(|| "All files".to_owned())
                                        });
                                        let trimmed = name.with(|n| n.trim().to_owned());
                                        let folder_name = if trimmed.is_empty() { "name".to_owned() } else { trimmed };
                                        format!("Creates {where_name} \u{203a} {folder_name} in Files and keeps its history.")
                                    }}
                                </small>
                            </div>
                            <label class="git-check">
                                <input
                                    type="checkbox"
                                    prop:checked=move || readme.get()
                                    on:change=move |ev| readme.set(event_target_checked(&ev))
                                />
                                "Add a README.md and make the first commit"
                            </label>
                        }.into_any()
                    } else {
                        view! {
                            <div class="git-field">
                                <span>"Folder"</span>
                                <FolderPicker
                                    initial_folder_id=""
                                    exclude_ids=Vec::new()
                                    disabled=busy
                                    on_change=Callback::new(move |sel| place.set(sel))
                                />
                                <small>
                                    {move || {
                                        place.with(|p| match p.as_ref().filter(|s| !s.id.is_empty()) {
                                            Some(sel) => format!("{} keeps its history from now on; its files are not changed.", sel.name),
                                            None => "Choose a folder you own.".to_owned(),
                                        })
                                    }}
                                </small>
                            </div>
                        }.into_any()
                    }
                }}
                {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                <div class="git-new-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                    <button type="submit" class="primary-button" disabled=move || !valid.get() || busy.get()>
                        {move || {
                            if busy.get() {
                                "Creating\u{2026}"
                            } else if mode.get() == "new" {
                                "Create"
                            } else {
                                "Keep history"
                            }
                        }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}
