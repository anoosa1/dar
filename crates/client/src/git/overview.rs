//! Git top-level landing views: Overview (heatmap + source mix + Needs attention + filtered cards
//! + clickable recent activity), Repositories list, cross-repo Pull requests, and Agent branches.

use std::collections::HashMap;

use js_sys::Date;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

use super::model::{
    API, GitActivity, GitActivitySources, GitAgentBranch, GitBranches, GitIssueList,
    GitIssueSummary, GitRepository, OverviewFilter, OverviewSort, Tab, encode_uri, plural,
    repo_api, use_load,
};
use super::parts::{
    BranchPair, Failure, GitSegmented, Loading, Person, Refs, RepoMark, Sha, SourceChip, Spark,
};
use crate::api::{self, Req, format_bytes};
use crate::dates::{edited, local_date};
use crate::icons::Icon;

#[component]
pub fn Overview(
    repos: Vec<GitRepository>,
    on_open: Callback<String>,
    on_open_tab: Callback<(String, Tab)>,
    on_open_commit: Callback<(String, String)>,
) -> impl IntoView {
    let activity = use_load::<GitActivity>(
        Signal::derive(move || Some(format!("{API}/activity"))),
        Signal::derive(|| 0u64),
    );
    let filter = RwSignal::new(OverviewFilter::All);
    let sort = RwSignal::new("updated".to_owned());

    let owned_count = repos.iter().filter(|r| r.role == "owner").count();
    let shared_count = repos.len().saturating_sub(owned_count);
    let dirty_repo = repos
        .iter()
        .filter(|r| r.dirty > 0)
        .max_by(|a, b| a.updated_at.total_cmp(&b.updated_at))
        .cloned();
    let pr_repo = repos
        .iter()
        .filter(|r| r.open_pulls > 0)
        .max_by_key(|r| r.open_pulls)
        .cloned();
    let agent_repo = repos
        .iter()
        .filter(|r| r.agent_branches > 0)
        .max_by_key(|r| r.agent_branches)
        .cloned();
    let has_attention = dirty_repo.is_some() || pr_repo.is_some() || agent_repo.is_some();

    let repo_count = repos.len();
    let repos_sv = StoredValue::new(repos);
    let sort_options = Signal::derive(|| {
        vec![
            ("updated".to_owned(), "Updated".to_owned()),
            ("name".to_owned(), "Name".to_owned()),
        ]
    });

    view! {
        <div class="git-heading-bar">
            <h1>"Overview"</h1>
            <div class="git-heading-end">
                <GitSegmented
                    label="Sort repositories"
                    value=Signal::derive(move || sort.get())
                    options=sort_options
                    on_change=Callback::new(move |next| sort.set(next))
                />
            </div>
        </div>
        <div class="git-scroll git-overview">
            {move || {
                let err = activity.error.get();
                if !err.is_empty() {
                    view! { <Failure error=activity.error retry=activity.retry what="Activity"/> }.into_any()
                } else if let Some(act) = activity.value.get() {
                    view! {
                        <Heatmap
                            times=act.times
                            sources=act.sources
                            repos=repo_count
                            owned=owned_count
                            shared=shared_count
                        />
                    }.into_any()
                } else {
                    view! { <Loading label="Loading activity"/> }.into_any()
                }
            }}

            {has_attention.then(|| view! {
                <section class="git-attention" aria-label="Needs attention">
                    <h2 class="git-section-title">"Needs attention"</h2>
                    <div class="git-attention-strip">
                        {dirty_repo.map(|repo| {
                            let id = repo.id.clone();
                            let title = format!("{} \u{00b7} {}", repo.name, plural(repo.dirty as usize, "uncommitted file", None));
                            view! {
                                <button
                                    type="button"
                                    class="git-att-item"
                                    on:click=move |_| { on_open_tab.try_run((id.clone(), Tab::Code)); }
                                >
                                    <span class="git-att-icon is-warn" aria-hidden="true"><Icon name="pencil"/></span>
                                    <span class="git-att-text">
                                        <strong>{title}</strong>
                                        <small>"Edited in Dar \u{00b7} Commit in Files before pushing"</small>
                                    </span>
                                </button>
                            }
                        })}
                        {pr_repo.map(|repo| {
                            let id = repo.id.clone();
                            let title = format!("{} \u{00b7} {}", repo.name, plural(repo.open_pulls, "open pull request", None));
                            let sub = if repo.open_issues > 0 {
                                format!("Also {} open", plural(repo.open_issues, "issue", None))
                            } else {
                                "Ready for review & merge".to_owned()
                            };
                            view! {
                                <button
                                    type="button"
                                    class="git-att-item"
                                    on:click=move |_| { on_open_tab.try_run((id.clone(), Tab::Pulls)); }
                                >
                                    <span class="git-att-icon is-ok" aria-hidden="true"><Icon name="pull"/></span>
                                    <span class="git-att-text">
                                        <strong>{title}</strong>
                                        <small>{sub}</small>
                                    </span>
                                </button>
                            }
                        })}
                        {agent_repo.map(|repo| {
                            let id = repo.id.clone();
                            let title = format!("{} \u{00b7} {}", repo.name, plural(repo.agent_branches, "Agent branch", Some("Agent branches")));
                            view! {
                                <button
                                    type="button"
                                    class="git-att-item"
                                    on:click=move |_| { on_open_tab.try_run((id.clone(), Tab::Branches)); }
                                >
                                    <span class="git-att-icon is-agent" aria-hidden="true"><Icon name="agent"/></span>
                                    <span class="git-att-text">
                                        <strong>{title}</strong>
                                        <small>"Waiting for review or pull request"</small>
                                    </span>
                                </button>
                            }
                        })}
                    </div>
                </section>
            })}

            <div class="git-overview-grid">
                <section aria-label="Repositories">
                    <div class="git-section-bar">
                        <h2 class="git-section-title">"Repositories"</h2>
                        <div class="git-filter-chips" role="group" aria-label="Filter repositories">
                            {[
                                (OverviewFilter::All, "All"),
                                (OverviewFilter::Dirty, "Uncommitted"),
                                (OverviewFilter::Pulls, "Open PRs"),
                                (OverviewFilter::Shared, "Shared"),
                            ].into_iter().map(|(kind, label)| {
                                view! {
                                    <button
                                        type="button"
                                        class=move || if filter.get() == kind { "git-fchip is-active" } else { "git-fchip" }
                                        aria-pressed=move || (filter.get() == kind).to_string()
                                        on:click=move |_| filter.set(kind)
                                    >
                                        {label}
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                    <div class="git-cards">
                        {move || {
                            let f = filter.get();
                            let s = if sort.get() == "name" { OverviewSort::Name } else { OverviewSort::Updated };
                            let mut list: Vec<GitRepository> = repos_sv.with_value(|all| {
                                all.iter()
                                    .filter(|r| match f {
                                        OverviewFilter::All => true,
                                        OverviewFilter::Dirty => r.dirty > 0,
                                        OverviewFilter::Pulls => r.open_pulls > 0,
                                        OverviewFilter::Shared => r.role != "owner",
                                    })
                                    .cloned()
                                    .collect()
                            });
                            if s == OverviewSort::Name {
                                list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                            }
                            if list.is_empty() {
                                return view! {
                                    <p class="git-hint git-span-all">"No repositories match this filter."</p>
                                }.into_any();
                            }
                            list.into_iter().map(|repo| {
                                let id = repo.id.clone();
                                let desc_class = if repo.description.is_empty() { "git-description is-empty" } else { "git-description" };
                                let desc = repo.error.clone().unwrap_or_else(|| {
                                    if repo.description.is_empty() { "No README.md yet".to_owned() } else { repo.description.clone() }
                                });
                                let spark_label = format!("Commits per week for {}", repo.name);
                                let tracker_badge = (repo.open_pulls > 0 || repo.open_issues > 0).then(|| {
                                    format!("{} PR \u{00b7} {} Iss", repo.open_pulls, repo.open_issues)
                                });
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
                                            {tracker_badge.map(|badge| view! { <span class="git-card-tracker">{badge}</span> })}
                                            <Spark weeks=repo.weeks label=spark_label/>
                                        </span>
                                    </button>
                                }
                            }).collect_view().into_any()
                        }}
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
                                    let repo_id_row = event.repo_id.clone();
                                    let sha_row = event.sha.clone();
                                    let repo_id_btn = event.repo_id.clone();
                                    let sha_display = event.sha.clone();
                                    view! {
                                        <li>
                                            <button
                                                type="button"
                                                class="git-feed-row"
                                                on:click=move |_| {
                                                    on_open_commit.try_run((repo_id_row.clone(), sha_row.clone()));
                                                }
                                            >
                                                <Person name=event.author.clone() source=event.source.clone()/>
                                                <span class="git-feed-body">
                                                    <span class="git-feed-what">
                                                        <strong>{event.author}</strong>
                                                        " in "
                                                        <span class="git-feed-repo">{event.repo_name}</span>
                                                        <Refs refs=event.refs/>
                                                    </span>
                                                    <span class="git-feed-message">{event.subject}</span>
                                                    <span class="git-feed-meta">
                                                        <SourceChip source=event.source/>
                                                        <Sha sha=sha_display/>
                                                        <span>{edited(event.time)}</span>
                                                    </span>
                                                </span>
                                            </button>
                                        </li>
                                    }
                                    .into_any()
                                    .also(|_| { let _ = repo_id_btn; })
                                }).collect_view()}
                            </ol>
                        }.into_any()
                    }}
                </section>
            </div>
        </div>
    }
}

trait Also: Sized {
    fn also(self, f: impl FnOnce(&Self)) -> Self {
        f(&self);
        self
    }
}
impl<T> Also for T {}

struct HeatCell {
    time_ms: f64,
    count: usize,
    level: u8,
    future: bool,
}

/// A year of commits per local day, Sunday-first columns ending this week, with source mix bar.
#[component]
fn Heatmap(
    times: Vec<f64>,
    sources: GitActivitySources,
    repos: usize,
    owned: usize,
    shared: usize,
) -> impl IntoView {
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
        let day =
            Date::new_with_year_month_day(first_year, first_month, first_day_offset + step as i32);
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
    let source_sum = (sources.git + sources.dar + sources.agent).max(1) as f64;
    let w_git = (sources.git as f64 / source_sum) * 100.0;
    let w_dar = (sources.dar as f64 / source_sum) * 100.0;
    let w_agent = (sources.agent as f64 / source_sum) * 100.0;
    let has_sources = sources.git + sources.dar + sources.agent > 0;
    let tip = RwSignal::new("Hover a day to inspect commits".to_owned());

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

    let repo_scope = if shared > 0 {
        format!("{owned} owned \u{00b7} {shared} shared")
    } else {
        format!(
            "across {}",
            plural(repos, "repository", Some("repositories"))
        )
    };

    view! {
        <section class="git-heat-card" aria-label="Activity over the last year">
            <div class="git-heat-head">
                <strong>{format!("{} in the last year", plural(total, "commit", None))}</strong>
                {has_sources.then(|| {
                    let summary = format!(
                        "{} Git \u{00b7} {} Dar \u{00b7} {} Agent",
                        sources.git, sources.dar, sources.agent
                    );
                    let summary_title = summary.clone();
                    view! {
                        <span class="git-src-mix" title=summary_title>
                            <svg class="git-mix-bar" viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true">
                                <rect x="0" y="0" width=format!("{w_git:.2}") height="6" class="is-git"/>
                                <rect x=format!("{w_git:.2}") y="0" width=format!("{w_dar:.2}") height="6" class="is-dar"/>
                                <rect x=format!("{:.2}", w_git + w_dar) y="0" width=format!("{w_agent:.2}") height="6" class="is-agent"/>
                            </svg>
                            <span>{summary}</span>
                        </span>
                    }
                })}
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
                            let label = (!cell.future).then(|| {
                                format!(
                                    "{} on {}",
                                    plural(cell.count, "commit", None),
                                    local_date(cell.time_ms, &[("dateStyle", "medium")]),
                                )
                            });
                            let hover_label = label.clone();
                            view! {
                                <i
                                    class=cls
                                    title=label
                                    on:mouseenter=move |_| {
                                        if let Some(ref l) = hover_label {
                                            tip.set(l.clone());
                                        }
                                    }
                                ></i>
                            }
                        }).collect_view()}
                    </div>
                </div>
            </div>
            <div class="git-heat-tip">
                <span>{move || tip.get()}</span>
                <span>{repo_scope}</span>
            </div>
        </section>
    }
}

#[component]
pub fn RepositoryList(repos: Vec<GitRepository>, on_open: Callback<String>) -> impl IntoView {
    let count = repos.len();
    view! {
        <div class="git-heading-bar">
            <h1>"Repositories"</h1>
            <span class="git-heading-count">{count}</span>
        </div>
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
                                    {(repo.open_pulls > 0).then(|| view! {
                                        <span class="git-card-tracker"><Icon name="pull"/>{plural(repo.open_pulls, "open PR", None)}</span>
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
                                    {(repo.open_issues > 0).then(|| view! {
                                        <span><Icon name="issue"/>{plural(repo.open_issues, "issue", None)}</span>
                                    })}
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
pub fn AllPullRequests(
    repos: Vec<GitRepository>,
    on_open_pull: Callback<(String, u32)>,
) -> impl IntoView {
    let candidates: Vec<GitRepository> = repos.into_iter().filter(|r| r.open_pulls > 0).collect();
    let total_pulls: usize = candidates.iter().map(|r| r.open_pulls).sum();
    let list_sv = StoredValue::new(candidates);
    let rows: RwSignal<Option<Vec<(GitRepository, GitIssueSummary)>>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let attempt = RwSignal::new(0u64);

    Effect::new(move |_| {
        let _ = attempt.get();
        error.set(String::new());
        let list = list_sv.get_value();
        let abort = web_sys::AbortController::new().ok();
        let signal = abort.as_ref().map(|a| a.signal());
        if let Some(abort) = abort {
            let abort = send_wrapper::SendWrapper::new(abort);
            on_cleanup(move || abort.abort());
        }
        spawn_local(async move {
            let check = signal.clone();
            let mut all = Vec::new();
            for repo in list {
                let url = format!("{}/issues?kind=pull&state=open", repo_api(&repo.id));
                match api::request::<GitIssueList>(
                    &url,
                    Req {
                        signal: signal.clone(),
                        ..Default::default()
                    },
                )
                .await
                {
                    Ok(res) => {
                        for item in res.items {
                            all.push((repo.clone(), item));
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
            all.sort_by(|a, b| b.1.updated_at.total_cmp(&a.1.updated_at));
            if !check.as_ref().is_some_and(|s| s.aborted()) {
                let _ = rows.try_set(Some(all));
            }
        });
    });

    view! {
        <div class="git-heading-bar">
            <h1>"Pull requests"</h1>
            {(total_pulls > 0).then(|| view! { <span class="git-heading-count">{total_pulls}</span> })}
        </div>
        {move || {
            if !error.with(String::is_empty) {
                return view! {
                    <div class="git-scroll">
                        <Failure error=error retry=Callback::new(move |_| attempt.update(|n| *n += 1)) what="Pull requests"/>
                    </div>
                }.into_any();
            }
            let Some(list) = rows.get() else {
                return view! { <div class="git-scroll"><Loading label="Loading pull requests"/></div> }.into_any();
            };
            if list.is_empty() {
                return view! {
                    <div class="git-empty">
                        <span class="empty-icon"><Icon name="pull"/></span>
                        <h2>"No open pull requests"</h2>
                        <p>"Open a pull request inside any repository\u{2019}s Branches or Pull requests tab to discuss and merge work."</p>
                    </div>
                }.into_any();
            }
            view! {
                <ul class="git-scroll git-issue-list" aria-label="Pull requests">
                    {list.into_iter().map(|(repo, item)| {
                        let repo_id = repo.id.clone();
                        let number = item.number;
                        let pair = item.head.clone().zip(item.base.clone());
                        let is_agent = item.agent.unwrap_or(false);
                        view! {
                            <li>
                                <button
                                    type="button"
                                    class="git-issue-row"
                                    on:click=move |_| { on_open_pull.try_run((repo_id.clone(), number)); }
                                >
                                    <span class="git-state-icon is-open" aria-hidden="true"><Icon name="pull"/></span>
                                    <span class="git-issue-main">
                                        <span class="git-issue-title">
                                            <span>{item.title}</span>
                                        </span>
                                        <span class="git-issue-sub">
                                            <strong>{repo.name}</strong>
                                            {format!(" \u{00b7} #{} \u{00b7} opened {} by {}", item.number, edited(item.created_at), item.author)}
                                            {pair.map(|(h, b)| view! { " \u{00b7} "<BranchPair head=h base=b/> })}
                                            {is_agent.then(|| view! { <span class="git-source is-agent"><Icon name="agent"/>"Agent"</span> })}
                                        </span>
                                    </span>
                                    <span class="git-issue-end">
                                        {(item.comments > 0).then(|| view! {
                                            <span class="git-muted" title=plural(item.comments, "comment", None)>
                                                <Icon name="comment"/>{item.comments}
                                            </span>
                                        })}
                                    </span>
                                </button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            }.into_any()
        }}
    }
}

#[component]
pub fn AgentBranches(repos: Vec<GitRepository>, on_open: Callback<String>) -> impl IntoView {
    let with_branches: Vec<GitRepository> =
        repos.into_iter().filter(|r| r.agent_branches > 0).collect();
    let count = with_branches
        .iter()
        .map(|r| r.agent_branches)
        .sum::<usize>();
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
                    Req {
                        signal: signal.clone(),
                        ..Default::default()
                    },
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
            all.sort_by(|a, b| b.1.modified_at.total_cmp(&a.1.modified_at));
            if !check.as_ref().is_some_and(|s| s.aborted()) {
                let _ = rows.try_set(Some(all));
            }
        });
    });

    view! {
        <div class="git-heading-bar">
            <h1>"Agent branches"</h1>
            {(count > 0).then(|| view! { <span class="git-heading-count">{count}</span> })}
        </div>
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
