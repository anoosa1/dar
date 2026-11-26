//! Shared UI controls for the Git app (`git/parts.tsx`) plus `model` re-exports
//! preserved for external callers (`history.rs`, `agent/*`).

use std::collections::HashSet;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

pub use super::model::*;

use super::diff::{Diff, DiffMode};
use crate::api::{self, BrowserSession};
use crate::icons::Icon;
use crate::shell::Avatar;
use crate::ui::{document, listen, unique_id, use_floating, window};

#[component]
pub fn SourceChip(#[prop(into)] source: String) -> impl IntoView {
    let icon = source_icon(&source);
    let label = source_label(&source);
    view! {
        <span class=format!("git-source is-{source}")><Icon name=icon/>{label}</span>
    }
}

#[component]
pub fn Sha(#[prop(into)] sha: String) -> impl IntoView {
    let display = short(&sha);
    view! { <code class="git-sha" title=sha>{display}</code> }
}

#[component]
pub fn Person(#[prop(into)] name: String, #[prop(optional, into)] source: String) -> impl IntoView {
    if source == "agent" {
        view! { <span class="avatar git-agent-avatar" aria-hidden="true"><Icon name="agent"/></span> }
            .into_any()
    } else {
        view! { <Avatar name=name/> }.into_any()
    }
}

#[component]
pub fn RepoMark(
    #[prop(into)] name: String,
    #[prop(optional, into)] size: &'static str,
) -> impl IntoView {
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_owned());
    let class = format!("git-mark hue-{} {size}", hue(&name));
    view! { <span class=class aria-hidden="true">{initial}</span> }
}

/// Commits per week as bars; heights use fixed CSS classes scaled to the busiest week.
#[component]
pub fn Spark(weeks: Vec<usize>, #[prop(into)] label: String) -> impl IntoView {
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
pub fn Refs(refs: Vec<GitRef>) -> impl IntoView {
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

#[component]
pub fn BranchPair(#[prop(into)] head: String, #[prop(into)] base: String) -> impl IntoView {
    let sr = format!("{head} into {base}");
    view! {
        <span class="git-branch-pair">
            <code>{head}</code>
            <Icon name="arrowRight" class="git-pair-arrow"/>
            <code>{base}</code>
            <span class="sr-only">{sr}</span>
        </span>
    }
}

#[component]
pub fn Failure(
    #[prop(into)] error: Signal<String>,
    retry: Callback<()>,
    #[prop(into)] what: String,
) -> impl IntoView {
    view! {
        <div class="git-failure error" role="alert">
            <Icon name="alert"/>
            <div>
                <strong>{format!("{what} could not be loaded")}</strong>
                <span>{move || error.get()}</span>
            </div>
            <button class="secondary-button" on:click=move |_| retry.run(())>"Retry"</button>
        </div>
    }
}

#[component]
pub fn Loading(#[prop(into)] label: String) -> impl IntoView {
    view! { <div class="git-skeleton" role="status" aria-label=label><span></span><span></span><span></span></div> }
}

#[component]
pub fn SheetHead(
    title: AnyView,
    #[prop(optional)] children: Option<Children>,
    #[prop(optional)] expanded: Option<Signal<bool>>,
    #[prop(optional)] on_expand: Option<Callback<()>>,
    on_close: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="git-sheet-head">
            <button class="icon-button git-sheet-back" aria-label="Close" on:click=move |_| { on_close.try_run(()); }><Icon name="back"/></button>
            <h2>{title}</h2>
            {children.map(|c| c())}
            {on_expand.map(|toggle| {
                let is_exp = move || expanded.is_some_and(|e| e.get());
                view! {
                    <button
                        type="button"
                        class=move || if is_exp() { "icon-button git-sheet-expand is-active" } else { "icon-button git-sheet-expand" }
                        aria-label=move || if is_exp() { "Dock side panel" } else { "Expand side panel" }
                        aria-pressed=move || is_exp().to_string()
                        title=move || if is_exp() { "Restore panel width" } else { "Expand full width" }
                        on:click=move |_| { toggle.try_run(()); }
                    >
                        <Icon name=Signal::derive(move || if is_exp() { "fitWidth" } else { "expand" })/>
                    </button>
                }
            })}
            <button class="icon-button git-sheet-close" aria-label="Close" title="Close (Escape)" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button>
        </div>
    }
}

#[component]
pub fn FileChanges(
    files: Vec<GitFileChange>,
    #[prop(into)] wrap: Signal<bool>,
    #[prop(optional, into)] mode: Option<Signal<DiffMode>>,
    #[prop(optional)] viewed: Option<RwSignal<HashSet<String>>>,
    #[prop(optional)] open_files: Option<RwSignal<HashSet<String>>>,
) -> impl IntoView {
    let initial: HashSet<String> = files.iter().take(5).map(|f| f.path.clone()).collect();
    let open = open_files.unwrap_or_else(|| RwSignal::new(initial));
    let diff_mode = Signal::derive(move || mode.map(|m| m.get()).unwrap_or_default());

    view! {
        <div class="git-diffs">
            {files.into_iter().map(|file| {
                let path_toggle = file.path.clone();
                let path_open = file.path.clone();
                let path_body = file.path.clone();
                let path_view_check = file.path.clone();
                let path_view_toggle = file.path.clone();
                let heading = match &file.previous_path {
                    Some(prev) => format!("{prev} \u{2192} {}", file.path),
                    None => file.path.clone(),
                };
                let kind_class = format!("git-kind is-{}", file.kind);
                let kind_text = kind_label(&file.kind);
                let is_open = Memo::new(move |_| open.with(|set| set.contains(&path_open)));
                let stat = if file.binary {
                    view! { "Binary" }.into_any()
                } else {
                    view! {
                        <span class="git-added">{format!("+{}", file.added)}</span>
                        " "
                        <span class="git-removed">{format!("\u{2212}{}", file.removed)}</span>
                    }.into_any()
                };
                let patch = StoredValue::new(file.patch);
                let binary = file.binary;
                let truncated = file.truncated.unwrap_or(false);
                let renamed = file.kind == "renamed";
                let viewed_btn = viewed.map(|v_sig| {
                    let p_check = path_view_check.clone();
                    let p_toggle = path_view_toggle.clone();
                    let is_viewed = Memo::new(move |_| v_sig.with(|set| set.contains(&p_check)));
                    let collapse_target = p_toggle.clone();
                    view! {
                        <button
                            type="button"
                            class=move || if is_viewed.get() { "git-viewed-pill is-viewed" } else { "git-viewed-pill" }
                            aria-pressed=move || is_viewed.get().to_string()
                            on:click=move |ev| {
                                ev.stop_propagation();
                                let p = p_toggle.clone();
                                let c = collapse_target.clone();
                                let mut now_viewed = false;
                                v_sig.update(|set| {
                                    if !set.remove(&p) {
                                        set.insert(p);
                                        now_viewed = true;
                                    }
                                });
                                if now_viewed {
                                    open.update(|set| {
                                        set.remove(&c);
                                    });
                                }
                            }
                        >
                            {move || if is_viewed.get() { "\u{2713} Viewed" } else { "Viewed" }}
                        </button>
                    }
                });

                view! {
                    <section class="git-diff">
                        <div class=move || if is_open.get() { "git-diff-head-row is-open" } else { "git-diff-head-row" }>
                            <button
                                type="button"
                                class="git-diff-head"
                                aria-expanded=move || is_open.get().to_string()
                                on:click=move |_| {
                                    let p = path_toggle.clone();
                                    open.update(|set| {
                                        if !set.remove(&p) {
                                            set.insert(p);
                                        }
                                    });
                                }
                            >
                                {move || if is_open.get() {
                                    view! { <Icon name="chevron" class="is-open"/> }.into_any()
                                } else {
                                    view! { <Icon name="chevron"/> }.into_any()
                                }}
                                <span class="git-diff-path">{heading}</span>
                                <span class=kind_class>{kind_text}</span>
                                <span class="git-stat">{stat}</span>
                            </button>
                            {viewed_btn}
                        </div>
                        {move || {
                            if !is_open.get() {
                                return None;
                            }
                            Some(match patch.get_value() {
                                Some(source) => view! {
                                    <Diff source=source wrap=wrap mode=diff_mode label=format!("Changes to {path_body}")/>
                                }.into_any(),
                                None => {
                                    let hint = if binary {
                                        "Binary file; no line changes to show."
                                    } else if truncated {
                                        "This diff is too large to show here. Clone the repository to see it."
                                    } else if renamed {
                                        "Renamed without changes."
                                    } else {
                                        "No line changes."
                                    };
                                    view! { <p class="git-hint">{hint}</p> }.into_any()
                                }
                            })
                        }}
                    </section>
                }
            }).collect_view()}
        </div>
    }
}

/// Merging from the web: fast-forward, or a merge commit with multi-line commit message box.
#[component]
pub fn MergeBox(
    repo: GitRepository,
    compare: GitCompare,
    session: BrowserSession,
    on_merged: Callback<GitMergeResult>,
    #[prop(optional, into)] default_message: Option<String>,
) -> impl IntoView {
    let mode = RwSignal::new(if compare.fast_forward {
        "fast-forward".to_owned()
    } else {
        "merge".to_owned()
    });
    let text = RwSignal::new(String::new());
    let remove = RwSignal::new(compare.head != "main");
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let message_id = unique_id();

    let repo_id = StoredValue::new(repo.id.clone());
    let csrf = StoredValue::new(session.csrf_token.clone());
    let base = StoredValue::new(compare.base.clone());
    let head = StoredValue::new(compare.head.clone());
    let base_sha = StoredValue::new(compare.base_sha.clone());
    let head_sha = StoredValue::new(compare.head_sha.clone());
    let default_msg = StoredValue::new(default_message);
    let fast_forward = compare.fast_forward;
    let mergeable = compare.mergeable;
    let behind = compare.behind;

    let blocked = move || mode.get() == "merge" && !mergeable;
    let do_merge = move |_| {
        busy.set(true);
        error.set(String::new());
        let current_mode = mode.get_untracked();
        let typed = text.get_untracked();
        let trimmed = typed.trim().to_owned();
        let delete_branch = remove.get_untracked();
        let mut body = serde_json::json!({
            "base": base.get_value(),
            "head": head.get_value(),
            "baseSha": base_sha.get_value(),
            "headSha": head_sha.get_value(),
            "mode": current_mode,
            "deleteBranch": delete_branch,
        });
        if current_mode == "merge" {
            let msg = if !trimmed.is_empty() {
                Some(trimmed)
            } else {
                default_msg.get_value()
            };
            if let Some(m) = msg {
                body["message"] = m.into();
            }
        }
        let url = format!("{}/merge", repo_api(&repo_id.get_value()));
        let token = csrf.get_value();
        spawn_local(async move {
            match api::send::<GitMergeResult>(&url, "POST", Some(body), &token).await {
                Ok(result) => {
                    on_merged.try_run(result);
                }
                Err(cause) => {
                    let _ = error.try_set(cause.message);
                    let _ = busy.try_set(false);
                }
            }
        });
    };

    let options = Signal::derive(move || {
        vec![
            ("fast-forward".to_owned(), "Fast-forward".to_owned()),
            ("merge".to_owned(), "Merge commit".to_owned()),
        ]
    });
    let placeholder = default_msg.get_value().unwrap_or_else(|| {
        let b = base.get_value();
        let h = head.get_value();
        if b == "main" {
            format!("Merge branch '{h}'")
        } else {
            format!("Merge branch '{h}' into {b}")
        }
    });
    let base_is_main = compare.base == "main";
    let head_not_main = compare.head != "main";
    let base_label = compare.base.clone();
    let head_label = compare.head.clone();

    view! {
        <section class="git-merge" aria-label=format!("Merge {head_label} into {base_label}")>
            <h3 class="git-sheet-label">{format!("Merge into {base_label}")}</h3>
            <GitSegmented
                label="How to merge"
                value=mode
                options=options
                disabled=busy
                on_change=Callback::new(move |v| mode.set(v))
            />
            {move || {
                if mode.get() == "fast-forward" {
                    if fast_forward {
                        view! { <p class="git-hint">{format!("{} moves forward to {}; no new commit is made.", base.get_value(), head.get_value())}</p> }.into_any()
                    } else {
                        view! { <p class="git-hint">{format!("{} has {} that {} doesn\u{2019}t, so it can\u{2019}t fast-forward. Choose Merge commit.", base.get_value(), plural(behind, "commit", None), head.get_value())}</p> }.into_any()
                    }
                } else if mergeable {
                    let mid = message_id.clone();
                    let mid_for = mid.clone();
                    let ph = placeholder.clone();
                    view! {
                        <label class="git-field" for=mid_for>
                            "Message"
                            <textarea
                                id=mid
                                class="git-merge-textarea"
                                rows="3"
                                prop:value=move || text.get()
                                maxlength="2000"
                                placeholder=ph
                                on:input=move |e| text.set(event_target_value(&e))
                            />
                        </label>
                    }.into_any()
                } else {
                    view! {
                        <div class="git-banner">
                            <Icon name="alert"/>
                            <div>
                                <strong>"These branches conflict"</strong>
                                <span>{format!("Merge {} into {} with Git, resolve the conflicts and push; then merge here.", base.get_value(), head.get_value())}</span>
                            </div>
                        </div>
                    }.into_any()
                }
            }}
            {base_is_main.then(|| view! {
                <p class="git-hint">"The folder in Files changes with main. Files with uncommitted changes in Dar stop the merge until they are committed."</p>
            })}
            {head_not_main.then(|| {
                let h = head_label.clone();
                view! {
                    <label class="git-check">
                        <input
                            type="checkbox"
                            prop:checked=move || remove.get()
                            disabled=move || busy.get()
                            on:change=move |e| remove.set(event_target_checked(&e))
                        />
                        {format!("Delete {h} after merging")}
                    </label>
                }
            })}
            {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
            <button
                class="primary-button git-merge-button"
                disabled=move || busy.get() || blocked() || (mode.get() == "fast-forward" && !fast_forward)
                on:click=do_merge
            >
                <Icon name="merge"/>
                {move || {
                    if busy.get() {
                        "Merging\u{2026}".to_owned()
                    } else if mode.get() == "fast-forward" {
                        format!("Fast-forward {}", base.get_value())
                    } else {
                        format!("Merge into {}", base.get_value())
                    }
                }}
            </button>
        </section>
    }
}

// ---- Reactive segmented and select controls -----------------------------------------------------

#[component]
pub fn GitSegmented(
    #[prop(into)] label: Signal<String>,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    #[prop(optional, into)] disabled: Signal<bool>,
    on_change: Callback<String>,
) -> impl IntoView {
    let name = StoredValue::new(unique_id());
    view! {
        <div role="radiogroup" aria-label=move || label.get() class="ui-segmented">
            {move || {
                let group = name.get_value();
                options.get().into_iter().map(|(option, text)| {
                    let group = group.clone();
                    let opt_class = option.clone();
                    let opt_checked = option.clone();
                    let opt_change = option;
                    view! {
                        <label class=move || if value.get() == opt_class { "is-checked" } else { "" }>
                            <input
                                type="radio"
                                name=group
                                prop:checked=move || value.get() == opt_checked
                                disabled=move || disabled.get()
                                on:change=move |_| { on_change.try_run(opt_change.clone()); }
                            />
                            <span>{text}</span>
                        </label>
                    }
                }).collect_view()
            }}
        </div>
    }
}

fn listbox_items(panel: &web_sys::HtmlElement) -> Vec<web_sys::HtmlElement> {
    let list = panel
        .query_selector_all("[role=\"option\"]:not(:disabled)")
        .expect("selector");
    (0..list.length())
        .filter_map(|i| list.item(i)?.dyn_into().ok())
        .collect()
}

fn focus_option_by_key(items: &[web_sys::HtmlElement], key: &str) -> bool {
    if !["ArrowDown", "ArrowUp", "Home", "End"].contains(&key) || items.is_empty() {
        return false;
    }
    let active = document().active_element();
    let index = items
        .iter()
        .position(|item| Some(item.unchecked_ref::<web_sys::Element>()) == active.as_ref())
        .map(|i| i as i64)
        .unwrap_or(-1);
    let len = items.len() as i64;
    let next = match key {
        "Home" => 0,
        "End" => len - 1,
        "ArrowDown" => (index + 1 + len) % len,
        _ => (index - 1 + len) % len,
    };
    let _ = items[next as usize].focus();
    true
}

#[component]
pub fn GitSelect(
    #[prop(into)] options: Signal<Vec<(String, String)>>,
    #[prop(into)] value: Signal<String>,
    #[prop(into)] aria_label: Signal<String>,
    #[prop(optional, into)] class: String,
    #[prop(optional, into)] disabled: Signal<bool>,
    on_change: Callback<String>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let side = RwSignal::new("down");
    let root = NodeRef::<html::Div>::new();
    let button = NodeRef::<html::Button>::new();
    let list = NodeRef::<html::Div>::new();
    let id = unique_id();
    let options_id = format!("{id}-options");
    use_floating(open.into(), button, list, false, 5.0);

    let choose = move |next: String| {
        open.set(false);
        if let Some(b) = button.get_untracked() {
            let _ = b.focus();
        }
        {
            on_change.try_run(next);
        };
    };

    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let Some(panel) = list.get() else { return };
        listen(&document(), "pointerdown", false, move |event| {
            let target = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            if let Some(root) = root.get_untracked()
                && !root.contains(target.as_ref())
            {
                open.set(false);
            }
        });
        let selected = panel
            .query_selector("[aria-selected=\"true\"]")
            .ok()
            .flatten()
            .or_else(|| {
                panel
                    .query_selector("[role=\"option\"]:not(:disabled)")
                    .ok()
                    .flatten()
            });
        if let Some(el) = selected.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
            let _ = el.focus();
        }
    });

    view! {
        <div node_ref=root class=format!("ui-select {class}") data-side=move || side.get()>
            <button
                id=id
                node_ref=button
                type="button"
                role="combobox"
                aria-label=move || aria_label.get()
                aria-expanded=move || open.get().to_string()
                aria-haspopup="listbox"
                aria-controls=options_id.clone()
                disabled=move || disabled.get()
                class="ui-select-trigger"
                on:click=move |_| {
                    let bottom = button.get_untracked().map(|b| b.get_bounding_client_rect().bottom()).unwrap_or(0.0);
                    let limit = window().inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) - 270.0;
                    side.set(if bottom > limit { "up" } else { "down" });
                    open.update(|o| *o = !*o);
                }
                on:keydown=move |e| {
                    if ["ArrowDown", "ArrowUp", " "].contains(&e.key().as_str()) {
                        e.prevent_default();
                        open.set(true);
                    } else if e.key() == "Escape" {
                        open.set(false);
                    }
                }
            >
                {move || {
                    let cur = value.get();
                    options
                        .with(|opts| opts.iter().find(|(v, _)| *v == cur).map(|(_, l)| l.clone()))
                        .unwrap_or_else(|| "Choose\u{2026}".to_owned())
                }}
                <Icon name="chevron"/>
            </button>
            <Show when=move || open.get()>
                <div
                    id=options_id.clone()
                    node_ref=list
                    role="listbox"
                    aria-label=move || aria_label.get()
                    class="ui-select-options"
                    on:keydown=move |e| {
                        let Some(panel) = list.get_untracked() else { return };
                        let items = listbox_items(&panel);
                        let key = e.key();
                        if key == "Escape" {
                            e.prevent_default();
                            e.stop_propagation();
                            open.set(false);
                            if let Some(b) = button.get_untracked() {
                                let _ = b.focus();
                            }
                        } else if focus_option_by_key(&items, &key) {
                            e.prevent_default();
                        } else if key == "Tab" {
                            open.set(false);
                        } else if key.chars().count() == 1 {
                            let lower = key.to_lowercase();
                            if let Some(item) = items.iter().find(|i| i.text_content().unwrap_or_default().to_lowercase().starts_with(&lower)) {
                                let _ = item.focus();
                            }
                        }
                    }
                >
                    {move || {
                        let cur = value.get();
                        options.get().into_iter().map(|(opt_val, text)| {
                            let selected = cur == opt_val;
                            let pick = opt_val.clone();
                            view! {
                                <button
                                    type="button"
                                    role="option"
                                    data-value=opt_val
                                    aria-selected=selected.to_string()
                                    on:click=move |_| choose(pick.clone())
                                >
                                    {text}
                                    {selected.then(|| view! { <Icon name="check"/> })}
                                </button>
                            }
                        }).collect_view()
                    }}
                </div>
            </Show>
        </div>
    }
}

// ---- Local Markdown with `#12` issue/PR reference links (pim/markdown.tsx) ----------------------

#[derive(Clone, Debug, PartialEq)]
enum Inline {
    Text(String),
    Code(String),
    Strong(String),
    Em(String),
    Link(String, String),
    Ref(u32),
}

fn is_content_link(target: &str) -> bool {
    let Some(rest) = target.strip_prefix("/api/v1/files/") else {
        return false;
    };
    let Some((id, tail)) = rest.split_once('/') else {
        return false;
    };
    !id.is_empty() && (tail == "content" || tail.starts_with("content?"))
}

fn md_token(source: &str, at: usize, allow_ref: bool) -> Option<(usize, Inline)> {
    let rest = &source[at..];
    let line_ok = |s: &str| !s.is_empty() && !s.contains('\n');
    if allow_ref && rest.starts_with('#') {
        let prev_blocks = at > 0
            && source[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '&' | '/' | '#'));
        if !prev_blocks {
            let digits = rest[1..].bytes().take_while(u8::is_ascii_digit).count();
            let next_word = rest[1 + digits..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            if (1..=6).contains(&digits)
                && !next_word
                && let Ok(number) = rest[1..1 + digits].parse::<u32>()
            {
                return Some((1 + digits, Inline::Ref(number)));
            }
        }
        return None;
    }
    if let Some(body) = rest.strip_prefix('`') {
        let close = body.find('`')?;
        return line_ok(&body[..close]).then(|| (close + 2, Inline::Code(body[..close].into())));
    }
    if let Some(body) = rest.strip_prefix("**")
        && let Some(close) = body.find("**")
        && line_ok(&body[..close])
        && !body[..close].contains('*')
    {
        return Some((close + 4, Inline::Strong(body[..close].into())));
    }
    if let Some(body) = rest.strip_prefix('*') {
        let close = body.find('*')?;
        return line_ok(&body[..close]).then(|| (close + 2, Inline::Em(body[..close].into())));
    }
    let (image, body) = match rest.strip_prefix("![") {
        Some(b) => (true, b),
        None => (false, rest.strip_prefix('[')?),
    };
    let close = body.find(']')?;
    let label = &body[..close];
    if label.contains('\n') {
        return None;
    }
    let target_part = body[close + 1..].strip_prefix('(')?;
    let end = target_part.find(|c: char| c == ')' || c.is_whitespace())?;
    if end == 0 || !target_part[end..].starts_with(')') {
        return None;
    }
    let target = &target_part[..end];
    let length = usize::from(image) + 1 + close + 2 + end + 1;
    let allowed =
        target.starts_with("http://") || target.starts_with("https://") || is_content_link(target);
    let raw = rest[..length].to_owned();
    Some((
        length,
        if !image && allowed {
            Inline::Link(label.into(), target.into())
        } else {
            Inline::Text(raw)
        },
    ))
}

fn md_inline(source: &str, allow_ref: bool) -> Vec<Inline> {
    let mut nodes = Vec::new();
    let (mut offset, mut at) = (0usize, 0usize);
    while at < source.len() {
        let rest = &source[at..];
        let b = rest.as_bytes()[0];
        if (matches!(b, b'`' | b'*' | b'[' | b'!') || (allow_ref && b == b'#'))
            && let Some((length, node)) = md_token(source, at, allow_ref)
        {
            if at > offset {
                nodes.push(Inline::Text(source[offset..at].into()));
            }
            nodes.push(node);
            at += length;
            offset = at;
            continue;
        }
        at += rest.chars().next().map(char::len_utf8).unwrap_or(1);
    }
    if offset < source.len() {
        nodes.push(Inline::Text(source[offset..].into()));
    }
    nodes
}

fn render_md_inline(source: &str, on_reference: Option<Callback<u32>>) -> AnyView {
    md_inline(source, on_reference.is_some())
        .into_iter()
        .map(|node| match node {
            Inline::Text(t) => t.into_any(),
            Inline::Code(t) => view! { <code>{t}</code> }.into_any(),
            Inline::Strong(t) => view! { <strong>{t}</strong> }.into_any(),
            Inline::Em(t) => view! { <em>{t}</em> }.into_any(),
            Inline::Link(label, href) => {
                let target = href.starts_with("http").then_some("_blank");
                view! { <a href=href rel="noopener noreferrer" target=target>{label}</a> }.into_any()
            }
            Inline::Ref(number) => {
                if let Some(on_ref) = on_reference {
                    view! {
                        <button type="button" class="git-link git-ref-link" on:click=move |_| { on_ref.try_run(number); }>
                            {format!("#{number}")}
                        </button>
                    }.into_any()
                } else {
                    format!("#{number}").into_any()
                }
            }
        })
        .collect_view()
        .into_any()
}

fn md_list_item(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some((false, rest));
        }
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0)
        .then(|| {
            trimmed[digits..]
                .strip_prefix(". ")
                .map(|rest| (true, rest))
        })
        .flatten()
}

#[component]
pub fn GitMarkdown(
    #[prop(into)] source: Signal<String>,
    #[prop(optional)] on_reference: Option<Callback<u32>>,
) -> impl IntoView {
    move || {
        let src = source.get();
        let lines: Vec<&str> = src
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect();
        let mut nodes: Vec<AnyView> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i];
            if line.trim_start().starts_with("```") {
                let language = line.trim_start().trim_start_matches("```").to_owned();
                let mut code = Vec::new();
                i += 1;
                while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                    code.push(lines[i]);
                    i += 1;
                }
                i += 1;
                let label = if language.is_empty() {
                    "Code".to_owned()
                } else {
                    format!("{language} code")
                };
                nodes.push(
                    view! { <pre><code aria-label=label>{code.join("\n")}</code></pre> }.into_any(),
                );
                continue;
            }
            if line.trim().is_empty() {
                i += 1;
                continue;
            }
            let hashes = line.bytes().take_while(|b| *b == b'#').count();
            if (1..=6).contains(&hashes)
                && line[hashes..].starts_with(|c: char| c.is_whitespace())
                && !line[hashes..].trim().is_empty()
            {
                let text = render_md_inline(line[hashes..].trim_start(), on_reference);
                nodes.push(match hashes {
                    1 => view! { <h1>{text}</h1> }.into_any(),
                    2 => view! { <h2>{text}</h2> }.into_any(),
                    3 => view! { <h3>{text}</h3> }.into_any(),
                    4 => view! { <h4>{text}</h4> }.into_any(),
                    5 => view! { <h5>{text}</h5> }.into_any(),
                    _ => view! { <h6>{text}</h6> }.into_any(),
                });
                i += 1;
                continue;
            }
            if let Some(rest) = line.strip_prefix('>') {
                let text = render_md_inline(rest.strip_prefix(' ').unwrap_or(rest), on_reference);
                nodes.push(view! { <blockquote>{text}</blockquote> }.into_any());
                i += 1;
                continue;
            }
            if let Some((ordered, _)) = md_list_item(line) {
                let mut items: Vec<AnyView> = Vec::new();
                while let Some((_, text)) = lines.get(i).and_then(|l| md_list_item(l)) {
                    i += 1;
                    let check = text.strip_prefix('[').and_then(|t| {
                        let mark = t.chars().next()?;
                        (matches!(mark, ' ' | 'x' | 'X') && t[1..].starts_with(']'))
                            .then(|| (mark == ' ', t[2..].trim_start()))
                    });
                    items.push(match check {
                        Some((unchecked, rest)) => view! {
                            <li>
                                <span aria-label=if unchecked { "Unchecked" } else { "Checked" }>{if unchecked { "\u{2610}" } else { "\u{2611}" }}</span>
                                " "
                                {render_md_inline(rest, on_reference)}
                            </li>
                        }.into_any(),
                        None => view! { <li>{render_md_inline(text, on_reference)}</li> }.into_any(),
                    });
                }
                nodes.push(if ordered {
                    view! { <ol>{items}</ol> }.into_any()
                } else {
                    view! { <ul>{items}</ul> }.into_any()
                });
                continue;
            }
            nodes.push(view! { <p>{render_md_inline(line, on_reference)}</p> }.into_any());
            i += 1;
        }
        view! { <div class="markdown-preview">{nodes}</div> }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_and_references() {
        assert_eq!(format_int(0), "0");
        assert_eq!(format_int(1234), "1,234");
        assert_eq!(plural(1, "commit", None), "1 commit");
        assert_eq!(
            plural(2, "repository", Some("repositories")),
            "2 repositories"
        );
        assert_eq!(
            md_inline("Closes #1 and foo#2 and **255**.", true),
            vec![
                Inline::Text("Closes ".into()),
                Inline::Ref(1),
                Inline::Text(" and foo#2 and ".into()),
                Inline::Strong("255".into()),
                Inline::Text(".".into()),
            ]
        );
    }
}
