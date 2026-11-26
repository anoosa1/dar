//! Top-level Git workspace shell: connects sidebar navigation, repository view,
//! right sheets (Commit, Compare, Issue), full-stage views, editing panes, settings,
//! URL search synchronization, and global keyboard shortcuts.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use super::branches::CompareSheet;
use super::commits::CommitSheet;
use super::diff::DiffMode;
use super::model::{
    API, GitPrefs, GitRepository, Notice, Sheet, TABS, Tab, View, blocked_shortcut_target,
    read_url_state, schedule_notice_clear, write_url_state,
};
use super::overview::{AgentBranches, AllPullRequests, Overview, RepositoryList};
use super::panes::NewRepository;
use super::repo::Repository;
use super::settings::GitSettings;
use super::sidebar::GitSidebar;
use super::tracker::{IssueEditing, IssueEditor, IssueSheet, LabelsEditor};
use crate::api::{self, BrowserSession, Req};
use crate::icons::Icon;
use crate::shell::WorkspaceHeader;
use crate::ui::{document, listen, window};

#[component]
pub fn GitWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let start = read_url_state();
    let initial_sheet = if let Some(number) = start.issue {
        Some(Sheet::Issue {
            number,
            is_pull: start.tab == Tab::Pulls,
        })
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
    let focus_mode = RwSignal::new(false);
    let settings_section: RwSignal<Option<&'static str>> = RwSignal::new(None);

    let prefs = GitPrefs::from_context();
    let diff_mode_str = RwSignal::new(match prefs.diff_layout.get_untracked() {
        DiffMode::Split => "split".to_owned(),
        DiffMode::Unified => "unified".to_owned(),
    });
    let wrap = RwSignal::new(prefs.wrap_lines.get_untracked());

    Effect::new(move |_| {
        diff_mode_str.set(match prefs.diff_layout.get() {
            DiffMode::Split => "split".to_owned(),
            DiffMode::Unified => "unified".to_owned(),
        });
    });
    Effect::new(move |_| {
        wrap.set(prefs.wrap_lines.get());
    });

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
                Req {
                    signal,
                    ..Default::default()
                },
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

    // Global keyboard shortcuts outside text fields, dialogs, menus, editors, and settings.
    listen(&document(), "keydown", false, move |event| {
        let event: web_sys::KeyboardEvent = event.unchecked_into();
        if event.default_prevented()
            || event.ctrl_key()
            || event.meta_key()
            || event.alt_key()
            || blocked_shortcut_target(&event)
        {
            return;
        }
        let key = event.key();
        match key.as_str() {
            "?" => {
                event.prevent_default();
                settings_section.set(Some("Keyboard shortcuts"));
            }
            "f" => {
                if repo_id.get_untracked().is_some() || sheet.with_untracked(Option::is_some) {
                    event.prevent_default();
                    focus_mode.update(|v| *v = !*v);
                }
            }
            "t" => {
                if repo_id.get_untracked().is_some()
                    && tab.get_untracked() == Tab::Code
                    && !matches!(sheet.get_untracked(), Some(Sheet::File { .. }))
                    && let Ok(Some(el)) = document().query_selector("#git-code-filter")
                    && let Ok(input) = el.dyn_into::<web_sys::HtmlElement>()
                {
                    event.prevent_default();
                    let _ = input.focus();
                }
            }
            "1" | "2" | "3" | "4" | "5" | "6" => {
                if repo_id.get_untracked().is_some()
                    && let Ok(idx) = key.parse::<usize>()
                    && let Some(&next_tab) = TABS.get(idx - 1)
                {
                    event.prevent_default();
                    tab.set(next_tab);
                    sheet.set(None);
                    focus_mode.set(false);
                }
            }
            "d" => {
                event.prevent_default();
                diff_mode_str.update(|cur| {
                    *cur = if cur == "split" {
                        "unified".to_owned()
                    } else {
                        "split".to_owned()
                    };
                });
            }
            "w" => {
                event.prevent_default();
                wrap.update(|v| *v = !*v);
            }
            "Escape" => {
                if focus_mode.get_untracked()
                    && editing.with_untracked(Option::is_none)
                    && !creating.get_untracked()
                    && settings_section.with_untracked(Option::is_none)
                    && sheet.with_untracked(Option::is_none)
                {
                    event.prevent_default();
                    focus_mode.set(false);
                }
            }
            _ => {}
        }
    });

    // Keep URL search params synchronized with current workspace state.
    Effect::new(move |_| {
        let current_view = view_mode.get();
        let current_repo = repo_id.get();
        let current_tab = tab.get();
        let current_ref = git_ref.get();
        let current_path = path.get();
        let current_sheet = sheet.get();
        write_url_state(
            current_view,
            current_repo.as_deref(),
            current_tab,
            &current_ref,
            &current_path,
            current_sheet.as_ref(),
        );
    });

    Effect::new(move |_| {
        if let Some(current) = notice.get() {
            schedule_notice_clear(notice, current.id);
        }
    });

    let notify = Callback::new(move |text: String| {
        let next_id = notice_seq.get_untracked() + 1;
        notice_seq.set(next_id);
        notice.set(Some(Notice {
            id: next_id,
            message: text,
        }));
    });

    let open_repo = Callback::new(move |(id, next_tab): (String, Tab)| {
        repo_id.set(Some(id));
        tab.set(next_tab);
        git_ref.set(String::new());
        path.set(String::new());
        sheet.set(None);
        editing.set(None);
        focus_mode.set(false);
    });

    let open_repo_commit = Callback::new(move |(id, sha): (String, String)| {
        repo_id.set(Some(id));
        tab.set(Tab::Commits);
        git_ref.set(String::new());
        path.set(String::new());
        sheet.set(Some(Sheet::Commit { sha }));
        editing.set(None);
        focus_mode.set(false);
    });

    let open_repo_pull = Callback::new(move |(id, number): (String, u32)| {
        repo_id.set(Some(id));
        tab.set(Tab::Pulls);
        git_ref.set(String::new());
        path.set(String::new());
        sheet.set(Some(Sheet::Issue {
            number,
            is_pull: true,
        }));
        editing.set(None);
        focus_mode.set(false);
    });

    let choose = Callback::new(move |next: View| {
        view_mode.set(next);
        repo_id.set(None);
        sheet.set(None);
        editing.set(None);
        focus_mode.set(false);
    });

    let current_repo = Memo::new(move |_| {
        let id = repo_id.get()?;
        repos.with(|list| list.as_ref()?.iter().find(|item| item.id == id).cloned())
    });

    let title = Memo::new(move |_| {
        if let Some(repo) = current_repo.get() {
            repo.name
        } else {
            view_mode.get().label().to_owned()
        }
    });

    // The issue drawer and editing pane are keyed by repository id, not its whole row,
    // so tracker count reloads don't remount an open draft.
    let current_repo_id =
        Memo::new(move |_| current_repo.with(|r| r.as_ref().map(|r| r.id.clone())));
    let repo_revision =
        Memo::new(move |_| current_repo.with(|r| r.as_ref().map_or(0, |r| r.revision)));
    let issue_version =
        Signal::derive(move || tracker_version.get().wrapping_add(repo_revision.get()));

    // Only right-hand drawer sheets (`Commit`, `Compare`, `Issue` drawer) activate `.has-pim-details`.
    // Full-stage `FileStage` and PR `IssueSheet` render inside `.git-panel`.
    let has_drawer = Memo::new(move |_| {
        current_repo.get().is_some()
            && matches!(
                sheet.get(),
                Some(
                    Sheet::Commit { .. }
                        | Sheet::Compare { .. }
                        | Sheet::Issue { is_pull: false, .. }
                )
            )
    });

    let session_sv = StoredValue::new(session.clone());
    let username = session.user.username.clone();

    view! {
        <div class=move || {
            let mut cls = "app-shell pim-shell git-shell".to_owned();
            if has_drawer.get() {
                cls.push_str(" has-pim-details");
            }
            if focus_mode.get() {
                cls.push_str(" is-git-focus");
            }
            cls
        }>
            <WorkspaceHeader
                app="git"
                session=session.clone()
                on_signed_out=on_signed_out
                on_settings=Callback::new(move |_| settings_section.set(Some("General")))
            />
            <div class="pim-workspace git-workspace">
                <GitSidebar
                    repos=repos
                    view_mode=view_mode
                    repo_id=repo_id
                    username=username
                    on_create=Callback::new(move |_| creating.set(true))
                    on_choose_view=choose
                    on_open_repo=open_repo
                    on_open_keys=Callback::new(move |_| settings_section.set(Some("HTTPS & keys")))
                />
                <main class="pim-main git-main" tabindex="-1" aria-label=move || title.get()>
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
                                        focus_mode.set(false);
                                    })
                                    git_ref=git_ref
                                    on_ref=Callback::new(move |r| git_ref.set(r))
                                    path=path
                                    on_path=Callback::new(move |p| path.set(p))
                                    sheet=sheet
                                    on_sheet=Callback::new(move |s| sheet.set(s))
                                    prefs=prefs
                                    diff_mode_str=diff_mode_str
                                    wrap=wrap
                                    focus_mode=focus_mode
                                    notify=notify
                                    on_changed=Callback::new(move |_| reload.update(|n| *n += 1))
                                    tracker_version=tracker_version
                                    issue_version=issue_version
                                    on_tracker_changed=tracker_changed
                                    on_edit=Callback::new(move |e| editing.set(Some(e)))
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
                                <Overview
                                    repos=list
                                    on_open=Callback::new(move |id| open_repo.run((id, Tab::Code)))
                                    on_open_tab=Callback::new(move |(id, t)| open_repo.run((id, t)))
                                    on_open_commit=open_repo_commit
                                />
                            }.into_any(),
                            View::Repositories => view! {
                                <RepositoryList
                                    repos=list
                                    on_open=Callback::new(move |id| open_repo.run((id, Tab::Code)))
                                />
                            }.into_any(),
                            View::Pulls => view! {
                                <AllPullRequests
                                    repos=list
                                    on_open_pull=open_repo_pull
                                />
                            }.into_any(),
                            View::Agent => view! {
                                <AgentBranches
                                    repos=list
                                    on_open=Callback::new(move |id| open_repo.run((id, Tab::Branches)))
                                />
                            }.into_any(),
                        }
                    }}
                </main>
                {move || {
                    let current = sheet.get()?;
                    if !matches!(
                        current,
                        Sheet::Commit { .. }
                            | Sheet::Compare { .. }
                            | Sheet::Issue { is_pull: false, .. }
                    ) {
                        return None;
                    }
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
                        Sheet::Issue { number, .. } => view! {
                            <IssueSheet
                                repo=repo
                                number=number
                                session=session_sv.get_value()
                                version=issue_version
                                diff_mode_str=diff_mode_str
                                wrap=wrap
                                full_stage=false
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
                                    sheet.set(None);
                                })
                                on_open=Callback::new(move |num| {
                                    sheet.set(Some(Sheet::Issue {
                                        number: num,
                                        is_pull: false,
                                    }));
                                })
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
                                diff_mode_str=diff_mode_str
                                wrap=wrap
                                expanded=focus_mode.into()
                                on_expand=Callback::new(move |_| focus_mode.update(|v| *v = !*v))
                                on_changed=Callback::new(move |_| reload.update(|n| *n += 1))
                                on_close=Callback::new(move |_| {
                                    focus_mode.set(false);
                                    sheet.set(None);
                                })
                                on_browse=Callback::new(move |sha| {
                                    tab.set(Tab::Code);
                                    git_ref.set(sha);
                                    path.set(String::new());
                                    sheet.set(None);
                                    focus_mode.set(false);
                                })
                                on_commit=Callback::new(move |sha| sheet.set(Some(Sheet::Commit { sha })))
                                notify=notify
                            />
                        }.into_any(),
                        Sheet::Compare { base, head } => view! {
                            <CompareSheet
                                repo=repo
                                session=session_sv.get_value()
                                diff_mode_str=diff_mode_str
                                wrap=wrap
                                notify=notify
                                expanded=focus_mode.into()
                                on_expand=Callback::new(move |_| focus_mode.update(|v| *v = !*v))
                                on_pull=Callback::new(move |(b, h)| {
                                    editing.set(Some(IssueEditing::Issue {
                                        kind: "pull".to_owned(),
                                        issue: None,
                                        base: Some(b),
                                        head: Some(h),
                                    }));
                                })
                                on_merged=Callback::new(move |_| {
                                    sheet.set(None);
                                    focus_mode.set(false);
                                    reload.update(|n| *n += 1);
                                })
                                base=base
                                head=head
                                on_change=Callback::new(move |(b, h)| sheet.set(Some(Sheet::Compare { base: b, head: h })))
                                on_commit=Callback::new(move |sha| sheet.set(Some(Sheet::Commit { sha })))
                                on_close=Callback::new(move |_| {
                                    focus_mode.set(false);
                                    sheet.set(None);
                                })
                            />
                        }.into_any(),
                        Sheet::File { .. } => ().into_any(),
                    };
                    Some(view! {
                        <aside
                            class=move || {
                                if focus_mode.get() {
                                    "pim-details git-details is-stage-details"
                                } else {
                                    "pim-details git-details"
                                }
                            }
                            aria-label=label
                        >
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
                                    sheet.set(Some(Sheet::Issue { number, is_pull }));
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
            {move || settings_section.get().map(|sec| view! {
                <GitSettings
                    session=session_sv.get_value()
                    initial_section=sec
                    on_close=Callback::new(move |_| settings_section.set(None))
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
