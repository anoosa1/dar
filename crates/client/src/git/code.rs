//! Repository Code explorer (tree, filter, latest commit, single-bordered README, About rail)
//! and full-stage inline File reader (`RefPicker`, `Code`, `FileStage`).

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use super::model::{
    GitCommitPage, GitPrefs, GitRepository, GitTree, RefLists, file_icon_and_class,
    file_language_label, focus_aside, is_markdown_path, plural, query_string, repo_api,
    sheet_keydown, short, use_load,
};
use super::parts::{Failure, GitSegmented, GitSelect, Loading, Person, Sha, copy};
use crate::api::format_bytes;
use crate::dates::edited;
use crate::icons::Icon;
use crate::markdown::Markdown;
use crate::ui::window;

#[component]
pub fn RefPicker(
    refs: RwSignal<Option<RefLists>>,
    #[prop(into)] value: Signal<String>,
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
            if all {
                "all".to_owned()
            } else {
                "main".to_owned()
            }
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
                    let _ = on_change.try_run(String::new());
                } else {
                    let _ = on_change.try_run(next);
                }
            })
        />
    }
}

#[component]
pub fn Code(
    repo: Signal<GitRepository>,
    refs: RwSignal<Option<RefLists>>,
    git_ref: RwSignal<String>,
    on_ref: Callback<String>,
    path: RwSignal<String>,
    on_path: Callback<String>,
    on_file: Callback<String>,
    on_commits: Callback<()>,
    on_commit: Callback<String>,
    on_pulls: Callback<()>,
    on_branches: Callback<()>,
    notify: Callback<String>,
    prefs: GitPrefs,
) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    Effect::new(move |_| {
        let _ = repo.with(|r| r.id.clone());
        let _ = path.get();
        let _ = git_ref.get();
        filter.set(String::new());
    });

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
        let ref_val = if current_ref.is_empty() {
            "main"
        } else {
            current_ref.as_str()
        };
        Some(format!(
            "{}/commits?{}",
            repo_api(&r.id),
            query_string(&[("ref", ref_val), ("limit", "1")]),
        ))
    });

    let tree = use_load::<GitTree>(
        tree_url,
        Signal::derive(move || repo.with(|r| r.revision as u64)),
    );
    let latest = use_load::<GitCommitPage>(
        latest_url,
        Signal::derive(move || repo.with(|r| r.revision as u64)),
    );

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
            let show_about = prefs.show_about.get();
            let repo_about = r.clone();

            view! {
                <div class="git-scroll git-code">
                    <div class="git-toolbar">
                        <RefPicker
                            refs=refs
                            value=Signal::derive(move || git_ref.get())
                            on_change=Callback::new(move |val| {
                                let _ = on_ref.try_run(val);
                                let _ = on_path.try_run(String::new());
                            })
                            label="Branch or tag"
                        />
                        <nav class="git-path" aria-label="Folder">
                            <button type="button" class="git-link" on:click=move |_| { let _ = on_path.try_run(String::new()); }>{r.name.clone()}</button>
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
                                            view! { <button type="button" class="git-link" on:click=move |_| { let _ = on_path.try_run(crumb_path.clone()); }>{part_text}</button> }.into_any()
                                        }}
                                    </span>
                                }
                            }).collect_view()}
                        </nav>
                        <span class="git-toolbar-end">
                            <label class="git-inline-input">
                                <Icon name="search"/>
                                <input
                                    id="git-code-filter"
                                    type="search"
                                    placeholder="Filter files\u{2026}"
                                    aria-label="Filter files"
                                    prop:value=move || filter.get()
                                    on:input=move |ev| filter.set(event_target_value(&ev))
                                />
                                <kbd aria-hidden="true">"t"</kbd>
                            </label>
                        </span>
                    </div>
                    {(r.dirty > 0 && current_ref.is_empty()).then(|| {
                        let files_href = if r.role == "owner" {
                            format!("/?folder={}&file={}", r.parent_id, r.id)
                        } else {
                            format!("/?folder={}", r.id)
                        };
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
                    <div class=move || if show_about { "git-code-grid" } else { "git-code-grid is-no-about" }>
                        <div class="git-code-main">
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
                                let raw_count = tree_data.entries.len();
                                let needle = filter.get().trim().to_ascii_lowercase();
                                let filtered_entries: Vec<_> = if needle.is_empty() {
                                    tree_data.entries
                                } else {
                                    tree_data
                                        .entries
                                        .into_iter()
                                        .filter(|e| e.name.to_ascii_lowercase().contains(&needle))
                                        .collect()
                                };
                                let no_entries = filtered_entries.is_empty();
                                let filter_active = !needle.is_empty();
                                view! {
                                    <div class="git-tree-card">
                                        {latest_page.and_then(|page| {
                                            let head = page.commits.into_iter().next()?;
                                            let sha = head.sha.clone();
                                            let total = page.total;
                                            Some(view! {
                                                <div class="git-latest">
                                                    <Person name=head.author.clone() source=head.source.clone()/>
                                                    <strong>{head.author}</strong>
                                                    <button type="button" class="git-link git-latest-message" on:click=move |_| { let _ = on_commit.try_run(sha.clone()); }>{head.subject}</button>
                                                    <Sha sha=head.sha/>
                                                    <span class="git-muted">{edited(head.time)}</span>
                                                    <button type="button" class="text-button" on:click=move |_| { let _ = on_commits.try_run(()); }>
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
                                                                <button type="button" class="git-file-name" on:click=move |_| { let _ = on_path.try_run(up.clone()); }>
                                                                    <Icon name="back"/>
                                                                    ".."
                                                                </button>
                                                            </td>
                                                        </tr>
                                                    }
                                                })}
                                                {filtered_entries.into_iter().map(|entry| {
                                                    let entry_path = entry.path.clone();
                                                    let is_tree = entry.kind == "tree";
                                                    let is_submodule = entry.kind == "submodule";
                                                    let (icon, cls) = file_icon_and_class(&entry.kind, &entry.name);
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
                                                                            let _ = on_path.try_run(entry_path.clone());
                                                                        } else {
                                                                            let _ = on_file.try_run(entry_path.clone());
                                                                        }
                                                                    }
                                                                >
                                                                    <Icon name=icon/>
                                                                    <span>{entry.name}</span>
                                                                </button>
                                                            </td>
                                                            <td class="git-file-commit">
                                                                {entry.last.map(|last| {
                                                                    let sha = last.sha.clone();
                                                                    view! {
                                                                        <button type="button" class="git-link" on:click=move |_| { let _ = on_commit.try_run(sha.clone()); }>{last.subject}</button>
                                                                    }
                                                                })}
                                                            </td>
                                                            <td class="git-file-time">{last_time}</td>
                                                        </tr>
                                                    }
                                                }).collect_view()}
                                            </tbody>
                                        </table>
                                    </div>
                                    {no_entries.then(|| {
                                        if raw_count > 0 && filter_active {
                                            view! {
                                                <p class="git-hint">
                                                    {format!("No files in this folder match \u{201c}{}\u{201d}. ", filter.get())}
                                                    <button type="button" class="git-link" on:click=move |_| filter.set(String::new())>"Clear filter"</button>
                                                </p>
                                            }.into_any()
                                        } else {
                                            view! { <p class="git-hint">"This folder is empty."</p> }.into_any()
                                        }
                                    })}
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
                        {show_about.then(|| {
                            let clone_url = repo_about.clone.alias.clone();
                            let clone_copy = repo_about.clone.alias.clone();
                            let desc = repo_about.description.trim().to_owned();
                            view! {
                                <aside class="git-about-rail" aria-label="About repository">
                                    <h3>"About"</h3>
                                    {if desc.is_empty() {
                                        view! { <p class="git-muted git-about-empty">"No folder description set in Files."</p> }.into_any()
                                    } else {
                                        view! { <p>{desc}</p> }.into_any()
                                    }}
                                    <div class="git-about-facts">
                                        <button type="button" class="git-about-fact" on:click=move |_| { let _ = on_commits.try_run(()); }>
                                            <Icon name="commit"/>
                                            <span>"Commits"</span>
                                            <strong>{repo_about.commits}</strong>
                                        </button>
                                        <button type="button" class="git-about-fact" on:click=move |_| { let _ = on_branches.try_run(()); }>
                                            <Icon name="branch"/>
                                            <span>"Branches"</span>
                                            <strong>{repo_about.branches}</strong>
                                        </button>
                                        {(repo_about.open_pulls > 0).then(|| view! {
                                            <button type="button" class="git-about-fact" on:click=move |_| { let _ = on_pulls.try_run(()); }>
                                                <Icon name="pull"/>
                                                <span>"Open pull requests"</span>
                                                <strong>{repo_about.open_pulls}</strong>
                                            </button>
                                        })}
                                        <div class="git-about-fact">
                                            <Icon name="archive"/>
                                            <span>"Repository size"</span>
                                            <strong>{format_bytes(repo_about.size)}</strong>
                                        </div>
                                        {(repo_about.tags > 0).then(|| view! {
                                            <div class="git-about-fact">
                                                <Icon name="tag"/>
                                                <span>"Tags"</span>
                                                <strong>{repo_about.tags}</strong>
                                            </div>
                                        })}
                                    </div>
                                    <button
                                        type="button"
                                        class="git-clone-mini"
                                        title="Copy HTTPS clone address"
                                        on:click=move |_| copy(clone_copy.clone(), notify, "clone address")
                                    >
                                        <Icon name="terminal"/>
                                        <span>{clone_url}</span>
                                        <Icon name="copy"/>
                                    </button>
                                </aside>
                            }
                        })}
                    </div>
                </div>
            }.into_any()
        }}
    }
}

#[derive(Clone)]
enum FileBody {
    Loading,
    Binary,
    Text(String),
}

/// Full-stage inline file reader rendered inside `.git-panel` when a file is open.
#[component]
pub fn FileStage(
    repo: GitRepository,
    git_ref: String,
    path: String,
    wrap: RwSignal<bool>,
    expanded: Signal<bool>,
    on_expand: Callback<()>,
    on_select_file: Callback<(String, String)>,
    on_browse_dir: Callback<(String, String)>,
    on_close: Callback<()>,
    notify: Callback<String>,
) -> impl IntoView {
    let dir_path = match path.rsplit_once('/') {
        Some((dir, _)) => dir.to_owned(),
        None => String::new(),
    };
    let active_dir = RwSignal::new(dir_path);
    let tree_open = RwSignal::new(true);
    let anchor_line = RwSignal::new(Option::<usize>::None);
    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
    let is_md = is_markdown_path(&name);
    let md_mode = RwSignal::new(if is_md {
        "rendered".to_owned()
    } else {
        "source".to_owned()
    });

    let repo_id = repo.id.clone();
    let tree_ref = git_ref.clone();
    let tree_url = Signal::derive(move || {
        let mut params = vec![("ref", tree_ref.clone())];
        let dir = active_dir.get();
        if !dir.is_empty() {
            params.push(("path", dir));
        }
        let pairs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        Some(format!(
            "{}/tree?{}",
            repo_api(&repo_id),
            query_string(&pairs)
        ))
    });
    let dir_tree = use_load::<GitTree>(tree_url, Signal::derive(|| 0u64));

    let url = format!(
        "{}/blob?{}",
        repo_api(&repo.id),
        query_string(&[("ref", git_ref.as_str()), ("path", path.as_str())])
    );
    let download_url = url.clone();
    let view_url = format!("{url}&view");
    let state = RwSignal::new(FileBody::Loading);
    let error = RwSignal::new(String::new());
    let root = NodeRef::<html::Aside>::new();
    focus_aside(root);

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
            let response: web_sys::Response =
                match fetched.and_then(|v| v.dyn_into::<web_sys::Response>()) {
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

    let name_contents = format!("Contents of {name}");
    let download_label = format!("Download {name}");
    let lang = file_language_label(&path);
    let edit_in_files_href = if repo.role == "owner" {
        format!("/?folder={}&file={}", repo.parent_id, repo.id)
    } else {
        format!("/?folder={}", repo.id)
    };
    let parts: Vec<String> = path.split('/').map(str::to_owned).collect();
    let parts_len = parts.len();
    let repo_name = repo.name.clone();
    let ref_for_tree_clicks = git_ref.clone();
    let ref_for_crumbs = git_ref.clone();
    let path_for_active = path.clone();

    let md_options = Signal::derive(move || {
        vec![
            ("rendered".to_owned(), "Rendered".to_owned()),
            ("source".to_owned(), "Source".to_owned()),
        ]
    });

    view! {
        <aside
            node_ref=root
            class="git-stage-pane git-file-stage"
            aria-label="File"
            tabindex="-1"
            on:keydown=move |ev| sheet_keydown(ev, on_close)
        >
            <div class=move || if tree_open.get() { "git-reader-layout" } else { "git-reader-layout is-tree-hidden" }>
                <div class="git-file-tree-pane" aria-label="Folder files">
                    <div class="git-file-tree-head">
                        <span>{move || {
                            let d = active_dir.get();
                            if d.is_empty() { repo_name.clone() } else { d }
                        }}</span>
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Hide file tree"
                            title="Hide file tree"
                            on:click=move |_| tree_open.set(false)
                        >
                            <Icon name="close"/>
                        </button>
                    </div>
                    {move || {
                        let d = active_dir.get();
                        (!d.is_empty()).then(|| {
                            let up = d.rsplit_once('/').map(|(p, _)| p.to_owned()).unwrap_or_default();
                            view! {
                                <button
                                    type="button"
                                    class="git-file-tree-item"
                                    on:click=move |_| active_dir.set(up.clone())
                                >
                                    <Icon name="back"/>
                                    <span>".."</span>
                                </button>
                            }
                        })
                    }}
                    {move || {
                        if !dir_tree.error.with(String::is_empty) {
                            return view! { <p class="git-muted git-file-tree-note">"Folder list unavailable"</p> }.into_any();
                        }
                        let Some(tree_data) = dir_tree.value.get() else {
                            return view! { <Loading label="Loading folder"/> }.into_any();
                        };
                        tree_data.entries.into_iter().map(|entry| {
                            let is_tree = entry.kind == "tree";
                            let is_active = entry.path == path_for_active;
                            let entry_path = entry.path.clone();
                            let r_click = ref_for_tree_clicks.clone();
                            let (icon, _) = file_icon_and_class(&entry.kind, &entry.name);
                            view! {
                                <button
                                    type="button"
                                    class=if is_active { "git-file-tree-item is-active" } else { "git-file-tree-item" }
                                    on:click=move |_| {
                                        if is_tree {
                                            active_dir.set(entry_path.clone());
                                        } else {
                                            let _ = on_select_file.try_run((r_click.clone(), entry_path.clone()));
                                        }
                                    }
                                >
                                    <Icon name=icon/>
                                    <span>{entry.name}</span>
                                </button>
                            }
                        }).collect_view().into_any()
                    }}
                </div>
                <div class="git-code-stage">
                    <div class="git-code-bar">
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Toggle file tree"
                            aria-pressed=move || tree_open.get().to_string()
                            title="Toggle file tree"
                            on:click=move |_| tree_open.update(|v| *v = !*v)
                        >
                            <Icon name="sidebar"/>
                        </button>
                        <nav class="git-code-path" aria-label="File path">
                            <button
                                type="button"
                                class="git-link"
                                on:click=move |_| { let _ = on_browse_dir.try_run((ref_for_crumbs.clone(), String::new())); }
                            >
                                {repo.name.clone()}
                            </button>
                            {parts.iter().enumerate().map(|(index, part)| {
                                let is_last = index == parts_len - 1;
                                let part_text = part.clone();
                                let crumb_path = parts[..=index].join("/");
                                let r_crumb = git_ref.clone();
                                view! {
                                    <span>
                                        <Icon name="chevron"/>
                                        {if is_last {
                                            view! { <strong>{part_text}</strong> }.into_any()
                                        } else {
                                            view! {
                                                <button
                                                    type="button"
                                                    class="git-link"
                                                    on:click=move |_| { let _ = on_browse_dir.try_run((r_crumb.clone(), crumb_path.clone())); }
                                                >
                                                    {part_text}
                                                </button>
                                            }.into_any()
                                        }}
                                    </span>
                                }
                            }).collect_view()}
                        </nav>
                        <span class="git-code-meta-pill">
                            {move || {
                                let extra = match state.get() {
                                    FileBody::Text(ref txt) => {
                                        let lines = txt.lines().count();
                                        format!(" \u{00b7} {} \u{00b7} {}", plural(lines, "line", None), format_bytes(txt.len() as f64))
                                    }
                                    _ => String::new(),
                                };
                                format!("{lang}{extra}")
                            }}
                        </span>
                        <span class="git-toolbar-end">
                            {is_md.then(|| view! {
                                <GitSegmented
                                    label="Markdown display"
                                    value=md_mode
                                    options=md_options
                                    on_change=Callback::new(move |val| md_mode.set(val))
                                />
                            })}
                            {move || (!is_md || md_mode.get() == "source").then(|| view! {
                                <label class="git-wrap">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || wrap.get()
                                        on:change=move |ev| wrap.set(event_target_checked(&ev))
                                    />
                                    "Wrap"
                                </label>
                            })}
                            {move || {
                                if let FileBody::Text(text) = state.get() {
                                    let to_copy = text.clone();
                                    Some(view! {
                                        <button
                                            type="button"
                                            class="icon-button"
                                            aria-label="Copy file contents"
                                            title="Copy file contents"
                                            on:click=move |_| copy(to_copy.clone(), notify, "file contents")
                                        >
                                            <Icon name="copy"/>
                                        </button>
                                    })
                                } else {
                                    None
                                }
                            }}
                            <a class="text-button" href=edit_in_files_href title="Open folder in Files">
                                <Icon name="pencil"/>
                                "Edit in Files"
                            </a>
                            <a class="icon-button" href=download_url download="" aria-label=download_label title="Download">
                                <Icon name="download"/>
                            </a>
                            <button
                                type="button"
                                class="icon-button"
                                aria-label=move || if expanded.get() { "Exit focus mode" } else { "Expand file reader" }
                                aria-pressed=move || expanded.get().to_string()
                                title=move || if expanded.get() { "Exit focus mode (f)" } else { "Focus mode (f)" }
                                on:click=move |_| { let _ = on_expand.try_run(()); }
                            >
                                <Icon name=Signal::derive(move || if expanded.get() { "fitWidth" } else { "expand" })/>
                            </button>
                            <button
                                type="button"
                                class="icon-button"
                                aria-label="Close"
                                title="Close (Esc)"
                                on:click=move |_| { let _ = on_close.try_run(()); }
                            >
                                <Icon name="close"/>
                            </button>
                        </span>
                    </div>
                    <div class="git-code-scroll">
                        {move || {
                            if !error.with(String::is_empty) {
                                return view! { <p class="error git-code-notice" role="alert">{error.get()}</p> }.into_any();
                            }
                            match state.get() {
                                FileBody::Loading => view! { <Loading label="Loading file"/> }.into_any(),
                                FileBody::Binary => view! {
                                    <p class="git-hint git-code-notice">"This file can\u{2019}t be shown here. Download it to open it."</p>
                                }.into_any(),
                                FileBody::Text(text) => {
                                    if is_md && md_mode.get() == "rendered" {
                                        view! {
                                            <div
                                                class="git-readme-body git-code-markdown"
                                                role="region"
                                                tabindex="0"
                                                aria-label=name_contents.clone()
                                            >
                                                <Markdown source=text/>
                                            </div>
                                        }.into_any()
                                    } else {
                                        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
                                        let is_empty = lines.is_empty();
                                        let wrap_cls = if wrap.get() {
                                            "git-code-well is-wrap"
                                        } else {
                                            "git-code-well"
                                        };
                                        view! {
                                            <div
                                                class=wrap_cls
                                                role="region"
                                                tabindex="0"
                                                aria-label=name_contents.clone()
                                            >
                                                {if is_empty {
                                                    view! { <p class="git-hint git-code-notice">"Empty file (0 bytes)."</p> }.into_any()
                                                } else {
                                                    lines.into_iter().enumerate().map(|(idx, line)| {
                                                        let ln = idx + 1;
                                                        view! {
                                                            <div class=move || if anchor_line.get() == Some(ln) { "git-code-line is-anchor" } else { "git-code-line" }>
                                                                <button
                                                                    type="button"
                                                                    class="git-code-ln"
                                                                    aria-label=format!("Line {ln}")
                                                                    on:click=move |_| {
                                                                        anchor_line.update(|cur| {
                                                                            *cur = if *cur == Some(ln) { None } else { Some(ln) };
                                                                        });
                                                                    }
                                                                >
                                                                    {ln}
                                                                </button>
                                                                <span class="git-code-txt">{line}</span>
                                                            </div>
                                                        }
                                                    }).collect_view().into_any()
                                                }}
                                            </div>
                                        }.into_any()
                                    }
                                }
                            }
                        }}
                    </div>
                </div>
            </div>
        </aside>
    }
}
