//! Git left sidebar: New repository CTA, global Git views (Overview, Repositories, Pull requests,
//! Agent branches), owned & shared repository lists with PR/dirty indicators, and compact HTTPS footer.

use leptos::prelude::*;

use super::model::{GitRepository, Tab, VIEWS, View, plural};
use super::parts::RepoMark;
use crate::icons::Icon;

#[component]
pub fn GitSidebar(
    repos: RwSignal<Option<Vec<GitRepository>>>,
    view_mode: RwSignal<View>,
    repo_id: RwSignal<Option<String>>,
    username: String,
    on_create: Callback<()>,
    on_choose_view: Callback<View>,
    on_open_repo: Callback<(String, Tab)>,
    on_open_keys: Callback<()>,
) -> impl IntoView {
    let agent_count = Memo::new(move |_| {
        repos.with(|list| {
            list.as_ref()
                .map(|items| items.iter().map(|i| i.agent_branches).sum::<usize>())
                .unwrap_or(0)
        })
    });
    let pulls_count = Memo::new(move |_| {
        repos.with(|list| {
            list.as_ref()
                .map(|items| items.iter().map(|i| i.open_pulls).sum::<usize>())
                .unwrap_or(0)
        })
    });

    view! {
        <aside class="sidebar git-sidebar" aria-label="Git navigation">
            <button type="button" class="new-button ui-new" on:click=move |_| { on_create.try_run(()); }>
                <Icon name="plus"/>
                "New repository"
            </button>
            <div class="git-side-scroll">
                <nav aria-label="Git views">
                    {VIEWS.into_iter().map(|v| {
                        let active = move || repo_id.get().is_none() && view_mode.get() == v;
                        view! {
                            <button
                                class=move || if active() { "nav-item active" } else { "nav-item" }
                                aria-current=move || active().then_some("page")
                                on:click=move |_| { on_choose_view.try_run(v); }
                            >
                                <Icon name=v.icon()/>
                                <span class="git-row-title">{v.label()}</span>
                                {move || {
                                    let count = match v {
                                        View::Repositories => repos.with(|r| r.as_ref().map(Vec::len).unwrap_or(0)),
                                        View::Pulls => pulls_count.get(),
                                        View::Agent => agent_count.get(),
                                        View::Overview => 0,
                                    };
                                    (count > 0).then(|| view! { <span class="git-count">{count}</span> })
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
                            let open_pulls = item.open_pulls;
                            view! {
                                <button
                                    class=move || if repo_id.get().as_deref() == Some(id_active.as_str()) { "nav-item active" } else { "nav-item" }
                                    aria-current=move || (repo_id.get().as_deref() == Some(id_aria.as_str())).then_some("page")
                                    on:click=move |_| { on_open_repo.try_run((id_click.clone(), Tab::Code)); }
                                >
                                    <RepoMark name=item.name.clone()/>
                                    <span class="git-row-title">{item.name}</span>
                                    {(open_pulls > 0).then(|| view! {
                                        <span class="git-count" title=plural(open_pulls, "open pull request", None)>{open_pulls}</span>
                                    })}
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
                                        on:click=move |_| { on_open_repo.try_run((id_click.clone(), Tab::Code)); }
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
            </div>

            <div class="git-side-foot">
                <button
                    type="button"
                    class="git-cred-row"
                    title="Manage Git HTTPS clone credentials and app passwords"
                    on:click=move |_| { on_open_keys.try_run(()); }
                >
                    <Icon name="key"/>
                    <span class="git-cred-label">
                        <strong>"Git HTTPS"</strong>
                        {format!(" \u{00b7} {username}")}
                    </span>
                    <span class="git-cred-tag">"Keys"</span>
                </button>
            </div>
        </aside>
    }
}
