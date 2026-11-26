//! Top-level T3-parity Agent pages and Searchable Settings modal (with native mobile drill-down):
//! - Usage & Limits page (styled Twilight progress bars, daily activity chart, provider cards)
//! - PRs & Issues hub (connected to Dar Git `/api/v1/git/repositories`)
//! - Automations / Scheduled tasks page (backed by `agentSchedules` preference)
//! - Searchable Agent Settings modal (left nav on desktop; full-width Hub -> Section on phones)

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use wasm_bindgen::JsCast;

use crate::api::{self, Req};
use crate::dates::local_date_time;
use crate::git::parts::{API as GIT_API, GitIssueList, GitIssueSummary, GitRepository, repo_api};
use crate::icons::Icon;
use crate::random_uuid;
use crate::settings::Notice;
use crate::ui::listen;

use super::surfaces::AgentScheduleEntry;

/// Top-level workspace pages selectable from the Agent sidebar utility dock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentPage {
    Workspace,
    Usage,
    PullRequests,
    Automations,
}

/// Lightweight info snapshots passed from `AgentWorkspace` into `pages.rs` without tight coupling.
#[derive(Clone, Debug, PartialEq)]
pub struct PageProviderInfo {
    pub id: String,
    pub name: String,
    pub configured: bool,
    pub connected: bool,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub is_default: bool,
    pub efforts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageLimitWindow {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageLimits {
    pub available: bool,
    pub plan: Option<String>,
    pub windows: Vec<PageLimitWindow>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageProjectInfo {
    pub id: String,
    pub name: String,
    pub source_type: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageThreadInfo {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub modified_at: f64,
}

/// Styled Twilight progress meter (CSP-safe: sets `--meter-pct` via DOM style property).
#[component]
pub fn StyledMeterBar(
    #[prop(into)] label: String,
    percent: f64,
    #[prop(optional, into)] subtitle: Option<String>,
) -> impl IntoView {
    let clamped = percent.clamp(0.0, 100.0);
    let level = if clamped >= 85.0 {
        "is-high"
    } else if clamped >= 60.0 {
        "is-medium"
    } else {
        "is-low"
    };
    let fill_ref = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if let Some(el) = fill_ref.get() {
            let _ = web_sys::HtmlElement::style(&el)
                .set_property("--meter-pct", &format!("{clamped:.1}%"));
        }
    });

    view! {
        <div class="agent-meter-row">
            <div class="agent-meter-head">
                <span class="agent-meter-label">{label.clone()}</span>
                <strong class=format!("agent-meter-val {level}")>{format!("{}%", clamped.round() as i64)}</strong>
            </div>
            <div class="agent-meter-track" role="progressbar" aria-label=label aria-valuemin="0" aria-valuemax="100" aria-valuenow=format!("{}", clamped.round() as i64)>
                <div node_ref=fill_ref class=format!("agent-meter-fill {level}")/>
            </div>
            {subtitle.filter(|s| !s.is_empty()).map(|s| view! { <small class="agent-meter-sub">{s}</small> })}
        </div>
    }
}

/// Full Usage & Limits page (accessible from the Agent sidebar dock).
#[component]
pub fn UsagePage(
    #[prop(into)] providers: Signal<Vec<PageProviderInfo>>,
    #[prop(into)] models: Signal<Vec<PageModelInfo>>,
    #[prop(into)] limits: Signal<Option<PageLimits>>,
    #[prop(into)] threads: Signal<Vec<PageThreadInfo>>,
    #[prop(into)] starred_models: Signal<Vec<String>>,
    #[prop(into)] checking: Signal<bool>,
    is_admin: bool,
    on_refresh: Callback<()>,
    on_connect: Callback<String>,
    on_toggle_star: Callback<String>,
    on_back: Callback<()>,
) -> impl IntoView {
    view! {
        <section class="agent-page" aria-label="Usage and limits">
            <header class="agent-page-head">
                <button type="button" class="icon-button" aria-label="Back to threads" on:click=move |_| { on_back.try_run(()); }><Icon name="back"/></button>
                <div>
                    <h1>"Usage & limits"</h1>
                    <p>"Live CLI rate limits, connected providers, and recent workspace activity."</p>
                </div>
                <button type="button" class="secondary-button" disabled=move || checking.get() on:click=move |_| { on_refresh.try_run(()); }>
                    <Icon name="refresh"/>
                    {move || if checking.get() { "Checking\u{2026}" } else { "Refresh" }}
                </button>
            </header>
            <div class="agent-page-scroll">
                <div class="agent-page-grid">
                    <article class="agent-page-card">
                        <h2>"Rate limits"</h2>
                        {move || match limits.get() {
                            None => view! { <p class="muted" role="status">"Checking usage limits\u{2026}"</p> }.into_any(),
                            Some(ref l) if !l.available => view! {
                                <p class="muted">{l.message.clone().unwrap_or_else(|| "Usage limits are unavailable for the active connection.".into())}</p>
                            }.into_any(),
                            Some(l) => view! {
                                {l.plan.map(|p| view! { <span class="agent-plan-pill">{format!("Plan: {p}")}</span> })}
                                <div class="agent-meter-list">
                                    {l.windows.into_iter().map(|w| {
                                        let sub = w.resets_at.map(|ts| {
                                            format!("Resets {}", local_date_time(ts, &[("weekday", "short"), ("hour", "numeric"), ("minute", "2-digit")]))
                                        }).unwrap_or_default();
                                        view! { <StyledMeterBar label=w.label percent=w.used_percent subtitle=sub/> }
                                    }).collect_view()}
                                </div>
                            }.into_any(),
                        }}
                    </article>
                    <article class="agent-page-card">
                        <h2>"Connected providers"</h2>
                        <div class="agent-provider-cards">
                            {move || providers.get().into_iter().map(|p| {
                                let id_conn = p.id.clone();
                                let name_conn = p.name.clone();
                                view! {
                                    <div class="agent-provider-card">
                                        <div class="agent-provider-card-head">
                                            <span class=if p.connected { "agent-connected" } else { "agent-status-dot" }/>
                                            <strong>{p.name}</strong>
                                            <span class="agent-provider-badge">{if p.connected { "Connected" } else if p.configured { "Unavailable" } else { "Not connected" }}</span>
                                        </div>
                                        <p class="muted small">{p.message}</p>
                                        {(is_admin && !p.configured).then(|| view! {
                                            <button type="button" class="primary-button" disabled=move || checking.get() on:click=move |_| { on_connect.try_run(id_conn.clone()); }>
                                                {format!("Connect server {name_conn}")}
                                            </button>
                                        })}
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    </article>
                </div>

                <article class="agent-page-card">
                    <h2>"7-day thread activity"</h2>
                    {move || {
                        let list = threads.get();
                        let now_ms = js_sys::Date::now();
                        let day_ms = 86_400_000.0;
                        let mut counts = [0usize; 7];
                        for t in &list {
                            let age_days = ((now_ms - t.modified_at) / day_ms).floor() as i64;
                            if (0..7).contains(&age_days) {
                                counts[6 - age_days as usize] += 1;
                            }
                        }
                        let max_c = counts.iter().copied().max().unwrap_or(1).max(1);
                        let labels = ["6d", "5d", "4d", "3d", "2d", "1d", "Today"];
                        view! {
                            <div class="agent-activity-bars" role="img" aria-label="Threads active over the last 7 days">
                                {counts.into_iter().enumerate().map(|(i, c)| {
                                    let pct = ((c as f64 / max_c as f64) * 100.0).max(if c > 0 { 16.0 } else { 6.0 });
                                    let bar_ref = NodeRef::<html::Div>::new();
                                    Effect::new(move |_| {
                                        if let Some(el) = bar_ref.get() {
                                            let _ = web_sys::HtmlElement::style(&el).set_property("--bar-h", &format!("{pct:.1}%"));
                                        }
                                    });
                                    let bar_cls = if c > 0 { "agent-activity-bar is-active" } else { "agent-activity-bar" };
                                    view! {
                                        <div class="agent-activity-col">
                                            <span class="agent-activity-num">{c}</span>
                                            <div class="agent-activity-track"><div node_ref=bar_ref class=bar_cls/></div>
                                            <span class="agent-activity-lbl">{labels[i]}</span>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }
                    }}
                </article>

                <article class="agent-page-card">
                    <h2>"Available models"</h2>
                    <p class="muted small">"Star models you use frequently to pin them to the top of the model picker."</p>
                    {move || {
                        let list = models.get();
                        let stars = starred_models.get();
                        if list.is_empty() {
                            return view! { <p class="muted">"Connect Codex or Claude Code to see available models."</p> }.into_any();
                        }
                        view! {
                            <ul class="agent-usage-models">
                                {list.into_iter().map(|m| {
                                    let is_star = stars.contains(&m.id);
                                    let id_star = m.id.clone();
                                    let eff = if m.efforts.is_empty() {
                                        "Standard".to_owned()
                                    } else {
                                        m.efforts.join(", ")
                                    };
                                    view! {
                                        <li>
                                            <button type="button" class=if is_star { "icon-button is-starred" } else { "icon-button" }
                                                aria-label=if is_star { format!("Unstar {}", m.name) } else { format!("Star {}", m.name) }
                                                aria-pressed=is_star.to_string()
                                                on:click=move |_| { on_toggle_star.try_run(id_star.clone()); }>
                                                <Icon name="star"/>
                                            </button>
                                            <div class="agent-usage-model-copy">
                                                <strong>{m.name}</strong>
                                                <span>{format!("{} \u{00b7} {eff}", m.provider)}</span>
                                            </div>
                                            {m.is_default.then(|| view! { <span class="agent-plan-pill">"Default"</span> })}
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }}
                </article>
            </div>
        </section>
    }
}

#[derive(Deserialize)]
struct GitRepoListResp {
    #[serde(default)]
    items: Vec<GitRepository>,
}

/// Full-canvas Pull Requests & Issues hub connected to Dar Git.
#[component]
pub fn PullRequestsPage(
    on_start_thread_prompt: Callback<String>,
    on_back: Callback<()>,
) -> impl IntoView {
    let rows: RwSignal<Vec<(String, String, GitIssueSummary)>> = RwSignal::new(Vec::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(String::new());
    let kind_filter = RwSignal::new("all");
    let search = RwSignal::new(String::new());
    let reload = RwSignal::new(0u64);

    Effect::new(move |_| {
        let _ = reload.get();
        let abort = web_sys::AbortController::new().expect("AbortController");
        let signal = abort.signal();
        let keep = SendWrapper::new(abort);
        on_cleanup(move || keep.abort());
        loading.set(true);
        error.set(String::new());
        spawn_local(async move {
            let repos = match api::request::<GitRepoListResp>(
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
            let mut out = Vec::new();
            for repo in repos.into_iter().take(20) {
                if signal.aborted() {
                    return;
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
                        out.push((repo.id.clone(), repo.name.clone(), item));
                    }
                }
            }
            if !signal.aborted() {
                out.sort_by(|a, b| b.2.updated_at.total_cmp(&a.2.updated_at));
                rows.try_set(out);
                loading.try_set(false);
            }
        });
    });

    view! {
        <section class="agent-page" aria-label="Pull requests and issues">
            <header class="agent-page-head">
                <button type="button" class="icon-button" aria-label="Back to threads" on:click=move |_| { on_back.try_run(()); }><Icon name="back"/></button>
                <div>
                    <h1>"Pull requests & issues"</h1>
                    <p>"Open pull requests and issues across your Dar Git repositories."</p>
                </div>
                <button type="button" class="secondary-button" on:click=move |_| reload.update(|n| *n += 1)><Icon name="refresh"/>"Refresh"</button>
            </header>
            <div class="agent-page-toolbar">
                <div class="agent-surface-pills" role="tablist">
                    {[("all", "All"), ("pull", "Pull requests"), ("issue", "Issues")].into_iter().map(|(k, lbl)| view! {
                        <button type="button" role="tab" aria-selected=move || (kind_filter.get() == k).to_string() on:click=move |_| kind_filter.set(k)>{lbl}</button>
                    }).collect_view()}
                </div>
                <input type="search" class="agent-page-search" aria-label="Filter pull requests and issues" placeholder="Filter by title, repo, or author\u{2026}"
                    prop:value=move || search.get()
                    on:input=move |e| search.set(event_target_value(&e))/>
            </div>
            <div class="agent-page-scroll">
                {move || {
                    let e = error.get();
                    if !e.is_empty() {
                        return view! { <p class="error" role="alert">{e}</p> }.into_any();
                    }
                    if loading.get() {
                        return view! { <p class="muted" role="status">"Loading Dar Git repositories\u{2026}"</p> }.into_any();
                    }
                    let fk = kind_filter.get();
                    let q = search.get().to_lowercase();
                    let filtered: Vec<(String, String, GitIssueSummary)> = rows
                        .get()
                        .into_iter()
                        .filter(|(_, rname, it)| {
                            (fk == "all" || it.kind == fk)
                                && (q.is_empty()
                                    || format!("{rname} {} {}", it.title, it.author).to_lowercase().contains(&q))
                        })
                        .collect();
                    if filtered.is_empty() {
                        return view! {
                            <div class="agent-review-empty">
                                <Icon name="git"/>
                                <h3>"Nothing open right now"</h3>
                                <p>"Create or sync repositories in Dar Git to review pull requests and issues alongside Agent threads."</p>
                                <a href="/git" class="primary-button">"Open Dar Git"</a>
                            </div>
                        }.into_any();
                    }
                    view! {
                        <div class="agent-pr-grid">
                            {filtered.into_iter().map(|(repo_id, repo_name, item)| {
                                let is_pr = item.kind == "pull";
                                let num = item.number;
                                let href = format!("/git?repo={repo_id}&view={}&item={num}", if is_pr { "pulls" } else { "issues" });
                                let prompt = format!("Review Dar Git repository `{repo_name}` {} #{num}: {}", if is_pr { "PR" } else { "issue" }, item.title);
                                view! {
                                    <article class="agent-page-card agent-pr-card">
                                        <div class="agent-pr-head">
                                            <span class=if is_pr { "agent-pr-badge is-pr" } else { "agent-pr-badge is-issue" }>
                                                {if is_pr { format!("PR #{num}") } else { format!("Issue #{num}") }}
                                            </span>
                                            <span class="agent-pr-repo">{repo_name}</span>
                                        </div>
                                        <h3>{item.title}</h3>
                                        <p class="muted small">{format!("Opened by {} \u{00b7} {} comments", item.author, item.comments)}</p>
                                        <div class="agent-pr-actions">
                                            <a href=href class="secondary-button">"Open in Git"</a>
                                            <button type="button" class="primary-button" on:click=move |_| { on_start_thread_prompt.try_run(prompt.clone()); }>"Work on this"</button>
                                        </div>
                                    </article>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </div>
        </section>
    }
}

/// Automations & Scheduled Tasks page (backed by `agentSchedules` preference).
#[component]
pub fn AutomationsPage(
    #[prop(into)] schedules: Signal<Vec<AgentScheduleEntry>>,
    #[prop(into)] projects: Signal<Vec<PageProjectInfo>>,
    on_save: Callback<Vec<AgentScheduleEntry>>,
    on_run_now: Callback<(String, String)>, // (project_id, prompt)
    on_back: Callback<()>,
) -> impl IntoView {
    let creating = RwSignal::new(false);
    let title = RwSignal::new(String::new());
    let project_id = RwSignal::new(String::new());
    let cadence = RwSignal::new("Daily at 09:00".to_owned());
    let prompt = RwSignal::new(String::new());

    let submit = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        let t = title.get_untracked().trim().to_owned();
        let p = prompt.get_untracked().trim().to_owned();
        let c = cadence.get_untracked().trim().to_owned();
        if t.is_empty() || p.is_empty() || c.is_empty() {
            return;
        }
        let pid = project_id.get_untracked();
        let mut next = schedules.get_untracked();
        if next.len() >= 16 {
            return;
        }
        next.push(AgentScheduleEntry {
            id: random_uuid(),
            title: t,
            project_id: pid,
            prompt: p,
            cadence: c,
            enabled: true,
        });
        on_save.try_run(next);
        title.set(String::new());
        prompt.set(String::new());
        creating.set(false);
    };

    view! {
        <section class="agent-page" aria-label="Automations">
            <header class="agent-page-head">
                <button type="button" class="icon-button" aria-label="Back to threads" on:click=move |_| { on_back.try_run(()); }><Icon name="back"/></button>
                <div>
                    <h1>"Automations"</h1>
                    <p>"Reusable prompt workflows and scheduled check-ins you can run in any project with one click."</p>
                </div>
                <button type="button" class="primary-button" disabled=move || schedules.with(|s| s.len() >= 16) on:click=move |_| creating.update(|v| *v = !*v)>
                    <Icon name="plus"/>
                    {move || if creating.get() { "Cancel" } else { "New automation" }}
                </button>
            </header>
            <div class="agent-page-scroll">
                {move || creating.get().then(|| {
                    let proj_list = projects.get();
                    view! {
                        <form class="agent-page-card agent-auto-form" on:submit=submit>
                            <h2>"Create automation"</h2>
                            <div class="agent-auto-row">
                                <label>
                                    "Title"
                                    <input type="text" required maxlength="100" placeholder="Morning repo triage"
                                        prop:value=move || title.get()
                                        on:input=move |e| title.set(event_target_value(&e))/>
                                </label>
                                <label>
                                    "Project"
                                    <select prop:value=move || project_id.get() on:change=move |e| project_id.set(event_target_value(&e))>
                                        <option value="">"Active / Scratch project"</option>
                                        {proj_list.into_iter().map(|p| view! { <option value=p.id>{p.name}</option> }).collect_view()}
                                    </select>
                                </label>
                                <label>
                                    "Cadence"
                                    <select prop:value=move || cadence.get() on:change=move |e| cadence.set(event_target_value(&e))>
                                        <option value="Daily at 09:00">"Daily at 09:00"</option>
                                        <option value="Weekdays at 09:00">"Weekdays at 09:00"</option>
                                        <option value="Every 6 hours">"Every 6 hours"</option>
                                        <option value="Weekly on Monday">"Weekly on Monday"</option>
                                        <option value="Manual run">"Manual run"</option>
                                    </select>
                                </label>
                            </div>
                            <label>
                                "Prompt"
                                <textarea rows="3" required maxlength="500" placeholder="Check recent changes, run tests, and summarize any failures or open TODOs."
                                    prop:value=move || prompt.get()
                                    on:input=move |e| prompt.set(event_target_value(&e))/>
                            </label>
                            <div class="dialog-actions">
                                <button type="button" class="secondary-button" on:click=move |_| creating.set(false)>"Cancel"</button>
                                <button type="submit" class="primary-button">"Save automation"</button>
                            </div>
                        </form>
                    }
                })}
                {move || {
                    let list = schedules.get();
                    let proj_list = projects.get();
                    if list.is_empty() && !creating.get() {
                        return view! {
                            <div class="agent-review-empty">
                                <Icon name="clock"/>
                                <h3>"No automations yet"</h3>
                                <p>"Create reusable audit, triage, or test-run prompts to launch in one click."</p>
                            </div>
                        }.into_any();
                    }
                    view! {
                        <div class="agent-pr-grid">
                            {list.into_iter().map(|item| {
                                let pname = proj_list
                                    .iter()
                                    .find(|p| p.id == item.project_id)
                                    .map(|p| p.name.clone())
                                    .unwrap_or_else(|| "Active project".into());
                                let id_toggle = item.id.clone();
                                let id_del = item.id.clone();
                                let pid_run = item.project_id.clone();
                                let prompt_run = item.prompt.clone();
                                view! {
                                    <article class=if item.enabled { "agent-page-card agent-auto-card" } else { "agent-page-card agent-auto-card is-paused" }>
                                        <div class="agent-pr-head">
                                            <span class="agent-plan-pill">{item.cadence}</span>
                                            <span class="agent-pr-repo">{pname}</span>
                                        </div>
                                        <h3>{item.title}</h3>
                                        <p class="muted small">{item.prompt}</p>
                                        <div class="agent-pr-actions">
                                            <button type="button" class="primary-button" on:click=move |_| { on_run_now.try_run((pid_run.clone(), prompt_run.clone())); }>
                                                <Icon name="play"/>"Run now"
                                            </button>
                                            <button type="button" class="secondary-button" on:click=move |_| {
                                                let mut next = schedules.get_untracked();
                                                if let Some(row) = next.iter_mut().find(|r| r.id == id_toggle) {
                                                    row.enabled = !row.enabled;
                                                }
                                                on_save.try_run(next);
                                            }>{if item.enabled { "Pause" } else { "Enable" }}</button>
                                            <button type="button" class="icon-button" aria-label="Delete automation" on:click=move |_| {
                                                let mut next = schedules.get_untracked();
                                                next.retain(|r| r.id != id_del);
                                                on_save.try_run(next);
                                            }><Icon name="trash"/></button>
                                        </div>
                                    </article>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </div>
        </section>
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SettingsSectionMeta {
    id: &'static str,
    label: &'static str,
    icon: &'static str,
    hint: &'static str,
    keywords: &'static str,
}

const SETTINGS_SECTIONS: &[SettingsSectionMeta] = &[
    SettingsSectionMeta {
        id: "General",
        label: "General",
        icon: "agent",
        hint: "Provider connections, limits & models",
        keywords: "codex claude provider connection limits usage models account",
    },
    SettingsSectionMeta {
        id: "Chat",
        label: "Chat",
        icon: "chat",
        hint: "Send key, follow-up mode, diff view & notifications",
        keywords: "send enter queue steer auto settle diff unified split notify",
    },
    SettingsSectionMeta {
        id: "Sidebar",
        label: "Sidebar",
        icon: "sidebar",
        hint: "Thread grouping and panel layout",
        keywords: "grouping recent project pinned sidebar layout",
    },
    SettingsSectionMeta {
        id: "Archived threads",
        label: "Archived threads",
        icon: "archive",
        hint: "Restore or permanently delete archived threads",
        keywords: "archived restore unarchive delete",
    },
    SettingsSectionMeta {
        id: "Keyboard shortcuts",
        label: "Keyboard shortcuts",
        icon: "sliders",
        hint: "Command palette, search, slash & navigation keys",
        keywords: "shortcuts hotkeys keyboard ctrl cmd enter escape",
    },
];

/// Searchable, categorized T3-parity Agent Settings dialog.
/// Desktop: left categorized nav + search + spacious content pane.
/// Mobile (<= 800px): native full-width Settings Hub -> Section drill-down (never cramped!).
#[component]
pub fn AgentSettingsModal(
    section: RwSignal<&'static str>,
    notice: Signal<Option<Notice>>,
    #[prop(into)] providers: Signal<Vec<PageProviderInfo>>,
    #[prop(into)] models: Signal<Vec<PageModelInfo>>,
    #[prop(into)] limits: Signal<Option<PageLimits>>,
    #[prop(into)] checking_provider: Signal<bool>,
    #[prop(into)] connection_error: Signal<String>,
    #[prop(into)] send_key: Signal<&'static str>,
    #[prop(into)] follow_up_mode: Signal<&'static str>,
    #[prop(into)] auto_settle: Signal<bool>,
    #[prop(into)] diff_view: Signal<&'static str>,
    #[prop(into)] grouping: Signal<&'static str>,
    #[prop(into)] notify_enabled: Signal<bool>,
    #[prop(into)] permission: Signal<String>,
    #[prop(into)] prefs_loaded: Signal<bool>,
    #[prop(into)] archived_threads: Signal<Vec<(PageThreadInfo, String)>>,
    is_admin: bool,
    user_id: String,
    on_close: Callback<()>,
    on_refresh_provider: Callback<()>,
    on_connect_provider: Callback<String>,
    on_update_prefs: Callback<Map<String, Value>>,
    on_request_notify: Callback<bool>,
    on_unarchive: Callback<String>,
    on_delete_archived: Callback<String>,
) -> impl IntoView {
    let search = RwSignal::new(String::new());
    // On mobile, start on the full-width Settings Hub (`false`), then drill into a section (`true`).
    let mobile_detail = RwSignal::new(false);
    let dialog_ref = NodeRef::<html::Dialog>::new();
    let user_id_sv = StoredValue::new(user_id);

    Effect::new(move |_| {
        if let Some(d) = dialog_ref.get()
            && !d.open()
        {
            let _ = d.show_modal();
        }
    });

    listen(&crate::ui::document(), "keydown", false, move |ev| {
        if let Ok(ke) = ev.dyn_into::<web_sys::KeyboardEvent>()
            && ke.key() == "Escape"
        {
            ke.prevent_default();
            if mobile_detail.get_untracked()
                && crate::ui::window()
                    .match_media("(max-width:800px)")
                    .ok()
                    .flatten()
                    .is_some_and(|m| m.matches())
            {
                mobile_detail.set(false);
            } else {
                on_close.try_run(());
            }
        }
    });

    let filtered_sections = Memo::new(move |_| {
        let q = search.get().trim().to_lowercase();
        SETTINGS_SECTIONS
            .iter()
            .copied()
            .filter(|s| {
                q.is_empty()
                    || s.label.to_lowercase().contains(&q)
                    || s.hint.to_lowercase().contains(&q)
                    || s.keywords.contains(&q)
            })
            .collect::<Vec<_>>()
    });

    view! {
        <dialog node_ref=dialog_ref
            class=move || if mobile_detail.get() { "agent-t3-settings is-mobile-detail" } else { "agent-t3-settings" }
            aria-label="Agent settings"
            on:cancel=move |e: web_sys::Event| { e.prevent_default(); on_close.try_run(()); }
            on:click=move |e: web_sys::MouseEvent| {
                if let Some(d) = dialog_ref.get_untracked()
                    && e.target().is_some_and(|t| t == *d.as_ref())
                {
                    on_close.try_run(());
                }
            }>
            <div class="agent-t3-settings-shell">
                <aside class="agent-t3-settings-nav">
                    <div class="agent-t3-settings-nav-head">
                        <span class="agent-t3-settings-brand"><Icon name="agent"/><strong>"Agent settings"</strong></span>
                        <button type="button" class="icon-button agent-t3-settings-close-mobile" aria-label="Close" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button>
                    </div>
                    <label class="agent-t3-settings-search">
                        <Icon name="search"/>
                        <span class="sr-only">"Search settings"</span>
                        <input type="search" placeholder="Search settings\u{2026}" prop:value=move || search.get() on:input=move |e| search.set(event_target_value(&e))/>
                    </label>
                    <div class="agent-t3-settings-tabs" role="tablist" aria-label="Agent settings sections">
                        {move || filtered_sections.get().into_iter().map(|meta| {
                            let active = move || section.get() == meta.id;
                            view! {
                                <button type="button" role="tab"
                                    class=move || if active() { "agent-t3-settings-tab is-active" } else { "agent-t3-settings-tab" }
                                    aria-selected=move || active().to_string()
                                    on:click=move |_| {
                                        section.set(meta.id);
                                        mobile_detail.set(true);
                                    }>
                                    <span class="agent-t3-settings-tab-ico"><Icon name=meta.icon/></span>
                                    <span class="agent-t3-settings-tab-copy">
                                        <strong>{meta.label}</strong>
                                        <small>{meta.hint}</small>
                                    </span>
                                    <Icon name="chevron" class="agent-t3-settings-tab-chev"/>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                    <p class="agent-t3-settings-foot">"Choices save to your Dar account automatically."</p>
                </aside>
                <div class="agent-t3-settings-body">
                    <header class="agent-t3-settings-body-head">
                        <button type="button" class="text-button agent-t3-settings-back" on:click=move |_| mobile_detail.set(false)>
                            <Icon name="back"/>"Settings"
                        </button>
                        <h2>{move || section.get()}</h2>
                        {move || notice.get().map(|n| view! {
                            <span class="agent-t3-settings-notice" role="status">{n.message}</span>
                        })}
                        <button type="button" class="icon-button" aria-label="Close" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button>
                    </header>
                    <div class="agent-t3-settings-pane" role="tabpanel">
                        {move || match section.get() {
                            "General" => {
                                let prov_list = providers.get();
                                let checking = checking_provider.get();
                                let cur_limits = limits.get();
                                let model_list = models.get();
                                let connected = prov_list.iter().any(|p| p.connected);
                                view! {
                                    <div class="agent-settings-stack">
                                        {prov_list.into_iter().map(|item| {
                                            let id_conn = item.id.clone();
                                            let name_lbl = item.name.clone();
                                            view! {
                                                <section class="agent-settings-card" aria-label=format!("{} connection", item.name)>
                                                    <div class="agent-provider-card-head">
                                                        <span class=if item.connected { "agent-connected" } else { "agent-status-dot" }/>
                                                        <h3>{format!("{} connection", item.name)}</h3>
                                                        <span class="agent-provider-badge">{if item.connected { "Connected" } else if item.configured { "Offline" } else { "Not linked" }}</span>
                                                    </div>
                                                    <p role="status" class="muted small">{if checking { format!("Checking {}\u{2026}", item.name) } else if item.message.is_empty() { "Connection could not be checked.".into() } else { item.message }}</p>
                                                    {(is_admin && !item.configured).then(|| view! {
                                                        <div class="agent-connection-actions">
                                                            <button type="button" class="primary-button" disabled=checking on:click=move |_| { on_connect_provider.try_run(id_conn.clone()); }>{format!("Connect server {name_lbl}")}</button>
                                                        </div>
                                                    })}
                                                </section>
                                            }
                                        }).collect_view()}
                                        {move || { let e = connection_error.get(); (!e.is_empty()).then(|| view! { <p class="error" role="alert">{e}</p> }) }}
                                        <div class="agent-connection-actions">
                                            <button type="button" class="secondary-button" disabled=checking on:click=move |_| { on_refresh_provider.try_run(()); }>
                                                <Icon name="refresh"/>
                                                {if checking { "Checking\u{2026}" } else { "Refresh connection" }}
                                            </button>
                                        </div>
                                        {connected.then(|| view! {
                                            <section class="agent-settings-card">
                                                <h3>"Usage limits"</h3>
                                                {match cur_limits {
                                                    None => view! { <p role="status" class="muted small">"Checking usage\u{2026}"</p> }.into_any(),
                                                    Some(ref l) if !l.available => view! { <p class="muted small">{l.message.clone().unwrap_or_else(|| "Usage limits are unavailable.".into())}</p> }.into_any(),
                                                    Some(l) => view! {
                                                        <div class="agent-meter-list">
                                                            {l.windows.into_iter().map(|w| {
                                                                let sub = w.resets_at.map(|ts| format!("Resets {}", local_date_time(ts, &[("weekday", "short"), ("hour", "numeric"), ("minute", "2-digit")]))).unwrap_or_default();
                                                                view! { <StyledMeterBar label=w.label percent=w.used_percent subtitle=sub/> }
                                                            }).collect_view()}
                                                            {l.plan.map(|plan| view! { <p class="muted small">{format!("Plan: {plan}")}</p> })}
                                                        </div>
                                                    }.into_any(),
                                                }}
                                            </section>
                                            <section class="agent-settings-card">
                                                <h3>"Models"</h3>
                                                <ul class="agent-settings-models">
                                                    {model_list.into_iter().map(|m| {
                                                        let eff_text = if m.efforts.is_empty() {
                                                            "No reasoning levels declared".into()
                                                        } else {
                                                            format!("Reasoning: {}", m.efforts.join(", "))
                                                        };
                                                        view! {
                                                            <li>
                                                                <strong>{m.name}</strong>
                                                                {m.is_default.then(|| view! { <small>"Default"</small> })}
                                                                <span>{format!("{} \u{00b7} {eff_text}", m.provider)}</span>
                                                            </li>
                                                        }
                                                    }).collect_view()}
                                                </ul>
                                            </section>
                                        })}
                                        <p class="muted small">"Runs execute inside isolated server sandboxes with no network access. Login tokens stay on the server."</p>
                                        <p class="muted small">"Dar account ID: "<code class="agent-settings-account">{user_id_sv.get_value()}</code></p>
                                    </div>
                                }.into_any()
                            }
                            "Chat" => {
                                let sk = send_key.get();
                                let fum = follow_up_mode.get();
                                let autosettle = auto_settle.get();
                                let dv = diff_view.get();
                                let loaded = prefs_loaded.get();
                                let notif = notify_enabled.get();
                                let perm = permission.get();
                                view! {
                                    <div class="agent-settings-stack">
                                        <section class="agent-settings-card">
                                            <h3>"Sending"</h3>
                                            <div class="agent-settings-choice" role="radiogroup" aria-label="Send messages with">
                                                {[
                                                    ("enter", "Enter sends, Shift Enter adds a line"),
                                                    ("modEnter", "Ctrl / Cmd Enter sends, Enter adds a line"),
                                                ].into_iter().map(|(val, label)| view! {
                                                    <label class="agent-radio-row">
                                                        <input type="radio" name="agent-send-key" prop:checked=sk == val disabled=!loaded on:change=move |_| {
                                                            let mut map = Map::new();
                                                            map.insert("agentSendKey".into(), json!(val));
                                                            on_update_prefs.try_run(map);
                                                        }/>
                                                        <span>{label}</span>
                                                    </label>
                                                }).collect_view()}
                                            </div>
                                        </section>
                                        <section class="agent-settings-card">
                                            <h3>"Follow-up behavior while running"</h3>
                                            <div class="agent-settings-choice" role="radiogroup" aria-label="Follow-up mode">
                                                {[
                                                    ("queue", "Queue follow-ups until the current turn finishes"),
                                                    ("steer", "Steer the active turn immediately"),
                                                ].into_iter().map(|(val, label)| view! {
                                                    <label class="agent-radio-row">
                                                        <input type="radio" name="agent-followup-mode" prop:checked=fum == val disabled=!loaded on:change=move |_| {
                                                            let mut map = Map::new();
                                                            map.insert("agentFollowUpMode".into(), json!(val));
                                                            on_update_prefs.try_run(map);
                                                        }/>
                                                        <span>{label}</span>
                                                    </label>
                                                }).collect_view()}
                                            </div>
                                        </section>
                                        <section class="agent-settings-card">
                                            <h3>"Diff viewer default"</h3>
                                            <div class="agent-settings-choice" role="radiogroup" aria-label="Default diff view">
                                                {[
                                                    ("unified", "Unified single-column diff"),
                                                    ("split", "Side-by-side split diff with inline comments"),
                                                ].into_iter().map(|(val, label)| view! {
                                                    <label class="agent-radio-row">
                                                        <input type="radio" name="agent-diff-view" prop:checked=dv == val disabled=!loaded on:change=move |_| {
                                                            let mut map = Map::new();
                                                            map.insert("agentDiffView".into(), json!(val));
                                                            on_update_prefs.try_run(map);
                                                        }/>
                                                        <span>{label}</span>
                                                    </label>
                                                }).collect_view()}
                                            </div>
                                        </section>
                                        <section class="agent-settings-card">
                                            <h3>"Workflow & notifications"</h3>
                                            <label class="agent-settings-toggle">
                                                <input type="checkbox" prop:checked=autosettle disabled=!loaded on:change=move |e| {
                                                    let mut map = Map::new();
                                                    map.insert("agentAutoSettle".into(), json!(event_target_checked(&e)));
                                                    on_update_prefs.try_run(map);
                                                }/>
                                                <span>"Auto-settle threads when a run finishes with no changes"</span>
                                            </label>
                                            <label class="agent-settings-toggle">
                                                <input type="checkbox" prop:checked=notif disabled=!loaded || perm == "unsupported" on:change=move |e| {
                                                    on_request_notify.try_run(event_target_checked(&e));
                                                }/>
                                                <span>"Notify me when an agent finishes in a background thread"</span>
                                            </label>
                                            {(perm == "denied").then(|| view! { <p class="muted small">"Notifications are blocked for this site in the browser."</p> })}
                                            {(perm == "unsupported").then(|| view! { <p class="muted small">"This browser does not support notifications."</p> })}
                                        </section>
                                    </div>
                                }.into_any()
                            }
                            "Sidebar" => {
                                let grp = grouping.get();
                                let loaded = prefs_loaded.get();
                                view! {
                                    <div class="agent-settings-stack">
                                        <section class="agent-settings-card">
                                            <h3>"Thread list"</h3>
                                            <div class="agent-settings-choice" role="radiogroup" aria-label="Thread list">
                                                {[
                                                    ("recent", "Recent first"),
                                                    ("project", "Grouped by project"),
                                                ].into_iter().map(|(val, label)| view! {
                                                    <label class="agent-radio-row">
                                                        <input type="radio" name="agent-grouping" prop:checked=grp == val disabled=!loaded on:change=move |_| {
                                                            let mut map = Map::new();
                                                            map.insert("agentGrouping".into(), json!(val));
                                                            on_update_prefs.try_run(map);
                                                        }/>
                                                        <span>{label}</span>
                                                    </label>
                                                }).collect_view()}
                                            </div>
                                            <p class="muted small">"Pinned threads stay at the top. Panel visibility and widths are remembered in this browser; double-click a resize handle to reset it."</p>
                                        </section>
                                    </div>
                                }.into_any()
                            }
                            "Archived threads" => {
                                let rows = archived_threads.get();
                                view! {
                                    <div class="agent-settings-stack">
                                        <section class="agent-settings-card">
                                            <h3>"Archived threads"</h3>
                                            {if rows.is_empty() {
                                                view! { <p class="muted">"No archived threads."</p> }.into_any()
                                            } else {
                                                view! {
                                                    <ul class="agent-archived">
                                                        {rows.into_iter().map(|(row, pname)| {
                                                            let id_u = row.id.clone();
                                                            let id_d = row.id.clone();
                                                            view! {
                                                                <li>
                                                                    <span class="agent-archived-proj">{pname}</span>
                                                                    <span>{row.title}</span>
                                                                    <button type="button" class="text-button" on:click=move |_| { on_unarchive.try_run(id_u.clone()); }>"Unarchive"</button>
                                                                    <button type="button" class="text-button danger" on:click=move |_| { on_delete_archived.try_run(id_d.clone()); }>"Delete"</button>
                                                                </li>
                                                            }
                                                        }).collect_view()}
                                                    </ul>
                                                }.into_any()
                                            }}
                                        </section>
                                    </div>
                                }.into_any()
                            }
                            _ => {
                                let sk = send_key.get();
                                view! {
                                    <div class="agent-settings-stack">
                                        <section class="agent-settings-card">
                                            <dl class="ui-shortcuts">
                                                <dt>"Ctrl / Cmd K"</dt><dd>"Search all apps"</dd>
                                                <dt>"Ctrl / Cmd P"</dt><dd>"Command palette: actions, threads, surfaces and projects"</dd>
                                                <dt>"Ctrl / Cmd Shift O"</dt><dd>"New thread"</dd>
                                                <dt>"Ctrl / Cmd B"</dt><dd>"Show or hide threads"</dd>
                                                <dt>"Ctrl / Cmd F"</dt><dd>"Find in loaded thread messages"</dd>
                                                <dt>{if sk == "enter" { "Enter" } else { "Ctrl / Cmd Enter" }}</dt><dd>"Run with the selected agent when connected; while running, follow up"</dd>
                                                <dt>{if sk == "enter" { "Shift Enter" } else { "Enter" }}</dt><dd>"Insert a new line"</dd>
                                                <dt>"/"</dt><dd>"Commands at the start of a message"</dd>
                                                <dt>"@"</dt><dd>"Mention a workspace file"</dd>
                                                <dt>"Up / Down"</dt><dd>"Previous prompts in an empty composer"</dd>
                                                <dt>"F2 / double-click"</dt><dd>"Rename the focused thread"</dd>
                                                <dt>"Escape"</dt><dd>"Close menus; return from a phone panel"</dd>
                                            </dl>
                                        </section>
                                    </div>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>
            </div>
        </dialog>
    }
}
