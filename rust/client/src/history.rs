//! Folder history in the folder details sheet (`folder-history.tsx`).

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::api::{self, format_bytes, BrowserSession, Req};
use crate::dates::local_date_time;
use crate::git::diff::{count_lines, text_lines, unified_hunks, Diff, DiffStat};
use crate::git::parts::{
    format_int, git_path, kind_label, plural, short, HistoryBranch, HistoryChange,
    HistoryChangeset, HistoryDetail, HistoryGitRef, HistoryOverview, HistoryStatus,
    HISTORY_LIMITS_DEPTH, HISTORY_LIMITS_FILES, HISTORY_LIMITS_MESSAGE_BYTES, HISTORY_LIMITS_REPOS,
};
use crate::icons::Icon;
use crate::ui::{unique_id, window, Dialog, EditorPane};

fn base(folder_id: &str) -> String {
    format!("/api/v1/files/history/{folder_id}")
}

/// Maps folder-history API error codes into user-facing copy (`historyFailure`).
pub fn history_failure(message: &str) -> String {
    let text = if message.is_empty() {
        "The operation could not be completed."
    } else {
        message
    };
    if text.contains("nothingToCommit") {
        return "There are no changes to commit.".to_owned();
    }
    if text.contains("uncommittedChanges") {
        return "Commit or snapshot the current changes before restoring.".to_owned();
    }
    if text.contains("conflict") {
        return "The folder or its history changed meanwhile. Review the latest state and try again.".to_owned();
    }
    if text.contains("operationTooLarge") {
        return format!(
            "History supports up to {} files and {} folder levels.",
            format_int(HISTORY_LIMITS_FILES),
            HISTORY_LIMITS_DEPTH
        );
    }
    if text.contains("quotaExceeded") {
        return "There isn\u{2019}t enough storage available for this history.".to_owned();
    }
    if text.contains("forbidden") {
        return "You can view this history but not change it.".to_owned();
    }
    if text.contains("notFound") {
        return "This history is no longer available, or you no longer have access.".to_owned();
    }
    if text.contains("contentUnavailable") {
        return "Some content in this history is unavailable. Try again later.".to_owned();
    }
    text.to_owned()
}

fn changeset_label(row: &HistoryChangeset) -> &'static str {
    match row.kind.as_str() {
        "snapshot" => "Snapshot",
        "restore" => "Restore",
        "merge" => "Merge",
        _ => "Commit",
    }
}

fn when(value: f64) -> String {
    local_date_time(value, &[("dateStyle", "medium"), ("timeStyle", "short")])
}

fn is_textual_extension(path: &str) -> bool {
    let Some(ext) = path.rsplit('.').next() else { return false };
    if ext.len() == path.len() {
        return false;
    }
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "md" | "markdown" | "txt" | "csv" | "json" | "yaml" | "yml" | "toml" | "xml"
            | "html" | "htm" | "css" | "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs"
            | "mts" | "cts" | "mjsx" | "cjsx" | "mtsx" | "ctsx"
            | "py" | "rb" | "go" | "rs" | "java" | "kt" | "swift" | "c" | "h" | "cpp"
            | "sh" | "ini" | "conf" | "env" | "sql" | "ics" | "vcf" | "svg" | "log"
    )
}

fn is_textual_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.contains("json")
        || mime.contains("xml")
        || mime.contains("javascript")
        || mime.contains("yaml")
}

/// Text that can be compared line by line in the browser.
fn textual(change: &HistoryChange) -> bool {
    let sizes_ok = change.size.unwrap_or(0.0) <= 512.0 * 1024.0
        && change.previous_size.unwrap_or(0.0) <= 512.0 * 1024.0;
    if !sizes_ok {
        return false;
    }
    if is_textual_extension(&change.path) {
        return true;
    }
    let mut types = Vec::new();
    if let Some(t) = change.media_type.as_deref().filter(|s| !s.is_empty()) {
        types.push(t);
    }
    if let Some(t) = change.previous_media_type.as_deref().filter(|s| !s.is_empty()) {
        types.push(t);
    }
    !types.is_empty() && types.iter().all(|t| is_textual_mime(t))
}

fn content_url(folder_id: &str, version_id: Option<&str>, file_id: Option<&str>) -> Option<String> {
    if let Some(vid) = version_id.filter(|s| !s.is_empty()) {
        Some(format!("{}/versions/{vid}/content", base(folder_id)))
    } else if let Some(fid) = file_id.filter(|s| !s.is_empty()) {
        Some(format!("/api/v1/files/{fid}/content"))
    } else {
        None
    }
}

async fn fetch_text(url: Option<String>, signal: &web_sys::AbortSignal) -> Result<String, &'static str> {
    let Some(url) = url else { return Ok(String::new()) };
    let init = web_sys::RequestInit::new();
    init.set_signal(Some(signal));
    let promise = window().fetch_with_str_and_init(&url, &init);
    let resp: web_sys::Response = JsFuture::from(promise)
        .await
        .map_err(|_| "failed")?
        .unchecked_into();
    if !resp.ok() {
        return Err("failed");
    }
    let text_promise = resp.text().map_err(|_| "failed")?;
    let text = JsFuture::from(text_promise)
        .await
        .map_err(|_| "failed")?
        .as_string()
        .unwrap_or_default();
    if text.contains('\0') {
        return Err("binary");
    }
    Ok(text)
}

#[derive(Clone, Debug)]
enum LoadedDiff {
    Patch { patch: String, added: usize, removed: usize },
    Note(String),
}

/// One path with its diff, loaded when expanded (`ChangeRow`).
#[component]
pub fn ChangeRow(
    #[prop(into)] folder_id: String,
    change: HistoryChange,
    live: bool,
    #[prop(optional, into)] selected: Option<Signal<bool>>,
    #[prop(optional)] on_select: Option<Callback<bool>>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let diff: RwSignal<Option<LoadedDiff>> = RwSignal::new(None);
    let wrap = RwSignal::new(false);

    let folder_sv = StoredValue::new(folder_id);
    let change_sv = StoredValue::new(change.clone());

    Effect::new(move |_| {
        if !open.get() || diff.with_untracked(Option::is_some) {
            return;
        }
        let ch = change_sv.get_value();
        if !textual(&ch) {
            let size_note = if ch.kind == "deleted" {
                format!("Was {}", format_bytes(ch.previous_size.unwrap_or(0.0)))
            } else {
                let mut s = format_bytes(ch.size.unwrap_or(0.0));
                if ch.kind == "modified"
                    && let Some(prev) = ch.previous_size
                {
                    s.push_str(&format!(", was {}", format_bytes(prev)));
                }
                s
            };
            diff.set(Some(LoadedDiff::Note(format!("{size_note}. No text preview for this file type."))));
            return;
        }
        let Ok(controller) = web_sys::AbortController::new() else { return };
        let signal = controller.signal();
        let abort_ref = SendWrapper::new(controller);
        let signal_check = SendWrapper::new(signal.clone());
        on_cleanup(move || abort_ref.take().abort());

        let fid = folder_sv.get_value();
        let old_url = if ch.kind == "added" {
            None
        } else {
            content_url(&fid, ch.previous_version_id.as_deref(), None)
        };
        let next_url = if ch.kind == "deleted" {
            None
        } else {
            content_url(
                &fid,
                ch.version_id.as_deref(),
                if live { ch.file_id.as_deref() } else { None },
            )
        };
        spawn_local(async move {
            let res = futures::future::try_join(
                fetch_text(old_url, &signal),
                fetch_text(next_url, &signal),
            )
            .await;
            if signal_check.aborted() {
                return;
            }
            match res {
                Ok((old, next)) => {
                    let a = text_lines(&old);
                    let b = text_lines(&next);
                    let patch = unified_hunks(&a, &b, &[]);
                    let (added, removed) = count_lines(&a, &b);
                    let _ = diff.try_set(Some(LoadedDiff::Patch { patch, added, removed }));
                }
                Err(code) => {
                    let note = if code == "binary" {
                        "No text preview for this file."
                    } else {
                        "The content could not be loaded. Close and open the file to try again."
                    };
                    let _ = diff.try_set(Some(LoadedDiff::Note(note.to_owned())));
                }
            }
        });
    });

    let path = change.path.clone();
    let prev_path = change.previous_path.clone();
    let kind_class = format!("history-change-kind is-{}", change.kind);
    let kind_text = kind_label(&change.kind);
    let path_for_label = path.clone();

    view! {
        <section class="history-change" aria-label=path.clone()>
            <header>
                {on_select.map(|cb| {
                    let include_label = format!("Include {path}");
                    view! {
                        <input
                            type="checkbox"
                            prop:checked=move || selected.map(|s| s.get()).unwrap_or(false)
                            aria-label=include_label
                            on:change=move |e| cb.run(event_target_checked(&e))
                        />
                    }
                })}
                <button
                    type="button"
                    class="history-change-toggle"
                    aria-expanded=move || open.get().to_string()
                    on:click=move |_| open.update(|v| *v = !*v)
                >
                    <Icon name="chevron"/>
                    <span class="history-change-path" title=path.clone()>
                        {path.clone()}
                        {prev_path.map(|prev| view! { <small>{format!("from {prev}")}</small> })}
                    </span>
                </button>
                <span class=kind_class>{kind_text}</span>
                {move || match diff.get() {
                    Some(LoadedDiff::Patch { added, removed, .. }) => Some(view! { <DiffStat added=added removed=removed/> }),
                    _ => None,
                }}
                {move || {
                    let has_patch = matches!(diff.get(), Some(LoadedDiff::Patch { .. }));
                    (open.get() && has_patch).then(|| view! {
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Wrap lines"
                            title="Wrap lines"
                            aria-pressed=move || wrap.get().to_string()
                            on:click=move |_| wrap.update(|v| *v = !*v)
                        >
                            <Icon name="menu"/>
                        </button>
                    })
                }}
            </header>
            {move || {
                if !open.get() {
                    return None;
                }
                let lbl = format!("Changes in {path_for_label}");
                Some(match diff.get() {
                    None => view! { <p class="history-note" role="status">"Loading changes\u{2026}"</p> }.into_any(),
                    Some(LoadedDiff::Note(note)) => view! { <p class="history-note">{note}</p> }.into_any(),
                    Some(LoadedDiff::Patch { patch, .. }) => {
                        if !patch.is_empty() {
                            view! { <Diff source=patch wrap=wrap label=lbl/> }.into_any()
                        } else {
                            view! { <p class="history-note">"The content is unchanged; only the name or place changed."</p> }.into_any()
                        }
                    }
                })
            }}
        </section>
    }
}

#[component]
fn CommitPane(
    folder_id: String,
    status: HistoryStatus,
    session: BrowserSession,
    on_close: Callback<()>,
    on_done: Callback<String>,
) -> impl IntoView {
    let message = RwSignal::new(String::new());
    let initial: HashSet<String> = status.changes.iter().map(|c| c.path.clone()).collect();
    let chosen = RwSignal::new(initial);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());

    let folder_sv = StoredValue::new(folder_id);
    let status_sv = StoredValue::new(status);
    let csrf = StoredValue::new(session.csrf_token);
    let dirty = Signal::derive(move || !message.with(|m| m.trim().is_empty()));

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        busy.set(true);
        error.set(String::new());
        let st = status_sv.get_value();
        let picked = chosen.get_untracked();
        let count = picked.len();
        let all = count == st.changes.len();
        let msg = message.get_untracked();
        let mut body = json!({ "message": msg, "token": st.token });
        if !all {
            let paths: Vec<String> = picked.into_iter().collect();
            body["paths"] = json!(paths);
        }
        let url = format!("{}/commits", base(&folder_sv.get_value()));
        let token = csrf.get_value();
        spawn_local(async move {
            match api::send::<serde_json::Value>(&url, "POST", Some(body), &token).await {
                Ok(_) => { on_done.try_run(format!("Committed {}.", plural(count, "change", None))); },
                Err(cause) => {
                    let _ = error.try_set(history_failure(&cause.message));
                    let _ = busy.try_set(false);
                }
            }
        });
    };

    view! {
        <EditorPane title="Commit changes" on_close=on_close dirty=dirty>
            <form class="history-form" on:submit=submit>
                <label>
                    "Message"
                    <textarea
                        name="message"
                        required
                        maxlength=HISTORY_LIMITS_MESSAGE_BYTES.to_string()
                        rows="3"
                        prop:value=move || message.get()
                        on:input=move |e| message.set(event_target_value(&e))
                        placeholder="Describe what changed"
                        autofocus
                    />
                </label>
                <div class="history-list-heading">
                    <strong>{move || format!("{} selected", plural(chosen.with(HashSet::len), "change", None))}</strong>
                    <button
                        type="button"
                        class="text-button"
                        on:click=move |_| {
                            let all_changes = status_sv.get_value().changes;
                            if chosen.with_untracked(HashSet::len) == all_changes.len() {
                                chosen.set(HashSet::new());
                            } else {
                                chosen.set(all_changes.into_iter().map(|c| c.path).collect());
                            }
                        }
                    >
                        {move || {
                            let total = status_sv.with_value(|s| s.changes.len());
                            if chosen.with(HashSet::len) == total { "Select none" } else { "Select all" }
                        }}
                    </button>
                </div>
                <div class="history-changes">
                    {move || {
                        let fid = folder_sv.get_value();
                        status_sv.get_value().changes.into_iter().map(|change| {
                            let path_sel = change.path.clone();
                            let path_cb = change.path.clone();
                            let sel = Signal::derive(move || chosen.with(|set| set.contains(&path_sel)));
                            let on_sel = Callback::new(move |val: bool| {
                                let p = path_cb.clone();
                                chosen.update(|set| {
                                    if val {
                                        set.insert(p);
                                    } else {
                                        set.remove(&p);
                                    }
                                });
                            });
                            view! { <ChangeRow folder_id=fid.clone() change=change live=true selected=sel on_select=on_sel/> }
                        }).collect_view()
                    }}
                </div>
                {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button">"Cancel"</button>
                    <button
                        class="primary-button"
                        disabled=move || busy.get() || chosen.with(HashSet::is_empty) || message.with(|m| m.trim().is_empty())
                    >
                        {move || if busy.get() { "Committing\u{2026}" } else { "Commit" }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}

#[component]
fn ChangesetPane(
    folder_id: String,
    changeset: HistoryChangeset,
    revision: u64,
    can_write: bool,
    session: BrowserSession,
    on_close: Callback<()>,
    on_done: Callback<String>,
) -> impl IntoView {
    let detail: RwSignal<Option<HistoryDetail>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let confirm = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let attempt = RwSignal::new(0u32);

    let folder_sv = StoredValue::new(folder_id);
    let changeset_sv = StoredValue::new(changeset.clone());
    let csrf = StoredValue::new(session.csrf_token);

    Effect::new(move |_| {
        let _ = attempt.get();
        let Ok(controller) = web_sys::AbortController::new() else { return };
        let signal = controller.signal();
        let abort_ref = SendWrapper::new(controller);
        let signal_check = SendWrapper::new(signal.clone());
        on_cleanup(move || abort_ref.take().abort());

        error.set(String::new());
        let url = format!("{}/commits/{}", base(&folder_sv.get_value()), changeset_sv.with_value(|c| c.id.clone()));
        spawn_local(async move {
            match api::request::<HistoryDetail>(&url, Req { signal: Some(signal), ..Default::default() }).await {
                Ok(d) => { let _ = detail.try_set(Some(d)); }
                Err(cause) => {
                    if !signal_check.aborted() {
                        let _ = error.try_set(history_failure(&cause.message));
                    }
                }
            }
        });
    });

    let restore = move |_| {
        busy.set(true);
        error.set(String::new());
        let cs = changeset_sv.get_value();
        let url = format!("{}/restore", base(&folder_sv.get_value()));
        let body = json!({ "id": cs.id, "revision": revision });
        let token = csrf.get_value();
        spawn_local(async move {
            match api::send::<serde_json::Value>(&url, "POST", Some(body), &token).await {
                Ok(_) => { on_done.try_run(format!("Folder restored to \u{201c}{}\u{201d}.", cs.message)); },
                Err(cause) => {
                    let _ = error.try_set(history_failure(&cause.message));
                    let _ = confirm.try_set(false);
                    let _ = busy.try_set(false);
                }
            }
        });
    };

    let title = changeset_label(&changeset);
    view! {
        <EditorPane title=title on_close=on_close>
            {move || {
                let cs = changeset_sv.get_value();
                view! {
                    <div class="history-summary">
                        <h3>{cs.message}</h3>
                        <p class="muted">
                            {format!("{} \u{00b7} {} \u{00b7} {}", cs.author, when(cs.created_at), plural(cs.file_count, "file", None))}
                            {cs.sha.map(|sha| {
                                let display = short(&sha);
                                view! { " \u{00b7} "<code class="history-sha" title=sha>{display}</code> }
                            })}
                        </p>
                    </div>
                }
            }}
            {move || (!error.get().is_empty()).then(|| view! {
                <div class="error" role="alert">
                    {error.get()}
                    {move || detail.with(Option::is_none).then(|| view! {
                        <button type="button" class="text-button" on:click=move |_| attempt.update(|v| *v += 1)>"Retry"</button>
                    })}
                </div>
            })}
            {move || match detail.get() {
                None if error.with(String::is_empty) => view! { <p class="history-note" role="status">"Loading changes\u{2026}"</p> }.into_any(),
                Some(d) => {
                    let fid = folder_sv.get_value();
                    view! {
                        <div class="history-changes">
                            {d.changes.into_iter().map(|change| view! {
                                <ChangeRow folder_id=fid.clone() change=change live=false/>
                            }).collect_view()}
                        </div>
                    }.into_any()
                }
                None => ().into_any(),
            }}
            {can_write.then(|| view! {
                <div class="dialog-actions">
                    <button type="button" class="secondary-button" disabled=move || busy.get() on:click=move |_| confirm.set(true)>
                        <Icon name="history"/>
                        "Restore folder to this point"
                    </button>
                </div>
            })}
            {move || confirm.get().then(|| view! {
                <Dialog title="Restore this folder?" on_close=Callback::new(move |_| confirm.set(false))>
                    <p>{format!(
                        "Files return to how they were at \u{201c}{}\u{201d}. Files added later move to deleted files. The restore is committed, so you can return to the current state from history.",
                        changeset_sv.with_value(|c| c.message.clone())
                    )}</p>
                    <div class="dialog-actions">
                        <button class="text-button" on:click=move |_| confirm.set(false)>"Cancel"</button>
                        <button class="primary-button" disabled=move || busy.get() on:click=restore>
                            {move || if busy.get() { "Restoring\u{2026}" } else { "Restore folder" }}
                        </button>
                    </div>
                </Dialog>
            })}
        </EditorPane>
    }
}

#[derive(Deserialize)]
struct BranchActionResp {
    #[allow(dead_code)]
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    conflicts: Vec<String>,
}

#[component]
fn BranchPane(
    folder_id: String,
    branch: HistoryBranch,
    revision: u64,
    can_write: bool,
    session: BrowserSession,
    on_close: Callback<()>,
    on_done: Callback<String>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let conflicts: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let confirm = RwSignal::new(false);

    let folder_sv = StoredValue::new(folder_id);
    let branch_sv = StoredValue::new(branch);
    let csrf = StoredValue::new(session.csrf_token);

    let act = move |action: &'static str| {
        busy.set(true);
        error.set(String::new());
        conflicts.set(Vec::new());
        let name = branch_sv.with_value(|b| b.name.clone());
        let url = format!("{}/branches/{action}", base(&folder_sv.get_value()));
        let body = json!({ "name": name, "revision": revision });
        let token = csrf.get_value();
        spawn_local(async move {
            match api::send::<BranchActionResp>(&url, "POST", Some(body), &token).await {
                Ok(res) => {
                    if !res.conflicts.is_empty() {
                        let _ = conflicts.try_set(res.conflicts);
                        let _ = busy.try_set(false);
                        let _ = confirm.try_set(false);
                        return;
                    }
                    { on_done.try_run(if action == "merge" {
                        "Agent changes merged and committed.".to_owned()
                    } else {
                        "Agent branch discarded.".to_owned()
                    }); };
                }
                Err(cause) => {
                    let _ = error.try_set(history_failure(&cause.message));
                    let _ = busy.try_set(false);
                    let _ = confirm.try_set(false);
                }
            }
        });
    };

    view! {
        <EditorPane title="Agent branch" on_close=on_close>
            {move || {
                let br = branch_sv.get_value();
                view! {
                    <div class="history-summary">
                        <h3>{br.message}</h3>
                        <p class="muted">{format!("{} \u{00b7} {} \u{00b7} {}", br.name, br.author, when(br.modified_at))}</p>
                    </div>
                }
            }}
            {move || {
                let list = conflicts.get();
                (!list.is_empty()).then(|| view! {
                    <div class="error" role="alert">
                        <p>"These files changed after the agent started, so nothing was merged:"</p>
                        <ul>
                            {list.into_iter().map(|path| view! { <li>{path}</li> }).collect_view()}
                        </ul>
                        <p>"Commit the agent\u{2019}s changes again from its thread, or discard this branch."</p>
                    </div>
                })
            }}
            {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
            <div class="history-changes">
                {move || {
                    let fid = folder_sv.get_value();
                    branch_sv.get_value().changes.into_iter().map(|change| view! {
                        <ChangeRow folder_id=fid.clone() change=change live=false/>
                    }).collect_view()
                }}
            </div>
            {can_write.then(|| view! {
                <div class="dialog-actions">
                    <button type="button" class="text-button danger" disabled=move || busy.get() on:click=move |_| confirm.set(true)>"Discard"</button>
                    <button type="button" class="primary-button" disabled=move || busy.get() on:click=move |_| act("merge")>
                        {move || if busy.get() { "Merging\u{2026}" } else { "Merge into folder" }}
                    </button>
                </div>
            })}
            {move || confirm.get().then(|| view! {
                <Dialog title="Discard this branch?" on_close=Callback::new(move |_| confirm.set(false))>
                    <p>{format!(
                        "The agent\u{2019}s proposed changes to {} are deleted. The folder is not changed.",
                        plural(branch_sv.with_value(|b| b.changes.len()), "file", None)
                    )}</p>
                    <div class="dialog-actions">
                        <button class="text-button" on:click=move |_| confirm.set(false)>"Cancel"</button>
                        <button class="danger-button" disabled=move || busy.get() on:click=move |_| act("discard")>"Discard branch"</button>
                    </div>
                </Dialog>
            })}
        </EditorPane>
    }
}

#[component]
fn GitCard(folder_id: String, refs: Vec<HistoryGitRef>, username: String) -> impl IntoView {
    let origin = window().location().origin().unwrap_or_default();
    let url = format!("{origin}{}", git_path(&folder_id));
    let copied = RwSignal::new(String::new());
    let field = unique_id();
    let url_copy = url.clone();
    let open_href = format!("/git?repo={folder_id}");

    view! {
        <div class="property-card history-git">
            <h3>"Git"</h3>
            <div class="history-clone">
                <label for=field.clone()>"Clone address"</label>
                <span class="history-clone-row">
                    <input
                        id=field
                        readonly=true
                        prop:value=url
                        on:focus=move |e: web_sys::FocusEvent| {
                            if let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                el.select();
                            }
                        }
                    />
                    <button
                        type="button"
                        class="secondary-button"
                        on:click=move |_| {
                            let text = url_copy.clone();
                            spawn_local(async move {
                                let promise = window().navigator().clipboard().write_text(&text);
                                match JsFuture::from(promise).await {
                                    Ok(_) => { let _ = copied.try_set("Address copied.".to_owned()); }
                                    Err(_) => { let _ = copied.try_set("Select the address and copy it.".to_owned()); }
                                }
                            });
                        }
                    >
                        <Icon name="copy"/>
                        "Copy"
                    </button>
                </span>
            </div>
            {move || (!copied.get().is_empty()).then(|| view! { <p class="muted small" role="status">{copied.get()}</p> })}
            <p class="muted small">{format!(
                "Sign in as {username} with an app password that has Files access. Pushing to main changes this folder; other branches and tags are kept without changing it."
            )}</p>
            {(!refs.is_empty()).then(|| view! {
                <ul class="history-refs" aria-label="Branches and tags">
                    {refs.into_iter().map(|r| {
                        let is_tag = r.name.starts_with("refs/tags/");
                        let display = r
                            .name
                            .strip_prefix("refs/heads/")
                            .or_else(|| r.name.strip_prefix("refs/tags/"))
                            .unwrap_or(&r.name)
                            .to_owned();
                        let sha_short = short(&r.sha);
                        view! {
                            <li>
                                <Icon name=if is_tag { "bookmark" } else { "share" }/>
                                <span title=r.name>{display}</span>
                                <small>{if is_tag { "Tag" } else { "Branch" }}</small>
                                <code>{sha_short}</code>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            })}
            <a class="text-button history-open-git" href=open_href>
                <Icon name="branch"/>
                "Open in Git"
            </a>
        </div>
    }
}

#[derive(Clone, Debug)]
enum ActivePane {
    Commit,
    Changeset(HistoryChangeset),
    Branch(HistoryBranch),
}

/// Folder history tab inside the Files folder details sheet (`FolderHistory`).
#[component]
pub fn FolderHistory(
    #[prop(into)] folder_id: Signal<String>,
    #[prop(into)] revision: Signal<u64>,
    session: BrowserSession,
    /// Kept by the caller when the sheet re-renders this tab, so a result notice survives it.
    #[prop(optional)] notice: Option<RwSignal<String>>,
) -> impl IntoView {
    let kept_notice = notice.is_some();
    let data: RwSignal<Option<HistoryOverview>> = RwSignal::new(None);
    let older: RwSignal<Vec<HistoryChangeset>> = RwSignal::new(Vec::new());
    let next: RwSignal<Option<u64>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let notice = notice.unwrap_or_else(|| RwSignal::new(String::new()));
    let busy = RwSignal::new(false);
    let pane: RwSignal<Option<ActivePane>> = RwSignal::new(None);
    let untrack = RwSignal::new(false);
    let sequence = RwSignal::new(0u64);

    let session_sv = StoredValue::new(session);

    let load = move || {
        let fid = folder_id.get_untracked();
        let current = sequence.get_untracked() + 1;
        sequence.set(current);
        let url = base(&fid);
        spawn_local(async move {
            match api::get::<HistoryOverview>(&url).await {
                Ok(value) => {
                    if sequence.try_get_untracked() != Some(current) {
                        return;
                    }
                    let nb = value.next_before;
                    let _ = data.try_set(Some(value));
                    let _ = older.try_set(Vec::new());
                    let _ = next.try_set(nb);
                    let _ = error.try_set(String::new());
                }
                Err(cause) => {
                    if sequence.try_get_untracked() == Some(current) {
                        let _ = error.try_set(history_failure(&cause.message));
                    }
                }
            }
        });
    };

    Effect::new(move |previous: Option<()>| {
        let _ = folder_id.get();
        data.set(None);
        if previous.is_some() || !kept_notice {
            notice.set(String::new());
        }
        pane.set(None);
        load();
    });

    Effect::new(move |_| {
        let _ = revision.get();
        if data.with_untracked(Option::is_some) {
            load();
        }
    });

    let post_act = move |path: &'static str, body: serde_json::Value, message: String| {
        busy.set(true);
        error.set(String::new());
        notice.set(String::new());
        let url = format!("{}{path}", base(&folder_id.get_untracked()));
        let token = session_sv.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            match api::send::<serde_json::Value>(&url, "POST", Some(body), &token).await {
                Ok(_) => {
                    let _ = notice.try_set(message);
                    load();
                }
                Err(cause) => {
                    let _ = error.try_set(history_failure(&cause.message));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let load_more = move |_| {
        let Some(before) = next.get_untracked() else { return };
        let url = format!("{}?before={before}", base(&folder_id.get_untracked()));
        spawn_local(async move {
            match api::get::<HistoryOverview>(&url).await {
                Ok(value) => {
                    let nb = value.next_before;
                    let _ = older.try_update(|cur| cur.extend(value.changesets));
                    let _ = next.try_set(nb);
                }
                Err(cause) => {
                    let _ = error.try_set(history_failure(&cause.message));
                }
            }
        });
    };

    let done = Callback::new(move |msg: String| {
        pane.set(None);
        notice.set(msg);
        load();
    });

    move || {
        let Some(overview) = data.get() else {
            return if !error.get().is_empty() {
                view! {
                    <div class="history-error" role="alert">
                        <p class="error">{error.get()}</p>
                        <button class="secondary-button" on:click=move |_| load()>"Retry"</button>
                    </div>
                }
                .into_any()
            } else {
                view! { <p class="muted" role="status">"Loading history\u{2026}"</p> }.into_any()
            };
        };

        let fid = folder_id.get_untracked();
        if !overview.tracked {
            let blocked = overview.blocked.clone();
            let owner = overview.owner;
            return view! {
                <div class="history-panel">
                    {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                    {move || (!notice.get().is_empty()).then(|| view! { <p class="history-notice" role="status"><Icon name="check"/>{notice.get()}</p> })}
                    <div class="empty-history">
                        <Icon name="history"/>
                        <h3>"Keep history for this folder"</h3>
                        <p>"Commit changes to files in this folder, see what changed and restore earlier states. Files keep working as usual; nothing is committed until you choose to."</p>
                        {match blocked.as_deref() {
                            Some("locked") => view! { <p class="muted">"Folders in the Locked folder can\u{2019}t keep history."</p> }.into_any(),
                            Some("nested") => view! { <p class="muted">"A folder that contains this one already keeps history."</p> }.into_any(),
                            Some("limit") => view! { <p class="muted">{format!("You can keep history for up to {HISTORY_LIMITS_REPOS} folders.")}</p> }.into_any(),
                            _ if owner => view! {
                                <button
                                    class="primary-button"
                                    disabled=move || busy.get()
                                    on:click=move |_| post_act("", json!({ "action": "track" }), "History is on for this folder.".to_owned())
                                >
                                    "Track history"
                                </button>
                            }.into_any(),
                            _ => view! { <p class="muted">"Only the owner can turn on history."</p> }.into_any(),
                        }}
                    </div>
                </div>
            }
            .into_any();
        }

        let status = overview.status.clone().unwrap_or(HistoryStatus { changes: Vec::new(), token: String::new() });
        let status_sv = StoredValue::new(status.clone());
        let repo_rev = overview.repo.as_ref().map(|r| r.revision).unwrap_or(0);
        let folder_name = StoredValue::new(overview.folder.name.clone());
        let can_write = overview.can_write;
        let owner = overview.owner;
        let branches = overview.branches.clone();
        let git_refs = overview.git_refs.clone();
        let initial_changesets = StoredValue::new(overview.changesets.clone());
        let username = session_sv.with_value(|s| s.user.username.clone());
        let bundle_href = format!("{}/bundle", base(&fid));
        let fid_sv = StoredValue::new(fid.clone());

        let pending_count = status.changes.len();
        let pending_preview: Vec<HistoryChange> = status.changes.iter().take(6).cloned().collect();
        let snap_token = status.token.clone();

        view! {
            <div class="history-panel">
                {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                {move || (!notice.get().is_empty()).then(|| view! { <p class="history-notice" role="status"><Icon name="check"/>{notice.get()}</p> })}
                <div class="property-card history-status">
                    <h3>"Uncommitted changes"</h3>
                    {if pending_count > 0 {
                        let snap_token = snap_token.clone();
                        view! {
                            <ul class="history-pending">
                                {pending_preview.into_iter().map(|change| {
                                    let path_title = change.path.clone();
                                    view! {
                                        <li>
                                            <span class=format!("history-change-kind is-{}", change.kind)>{kind_label(&change.kind)}</span>
                                            <span title=path_title>{change.path}</span>
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                            {(pending_count > 6).then(|| view! {
                                <p class="history-more">{format!("and {} more change{}", format_int(pending_count - 6), if pending_count - 6 == 1 { "" } else { "s" })}</p>
                            })}
                            {can_write.then(|| view! {
                                <div class="history-actions">
                                    <button class="primary-button" disabled=move || busy.get() on:click=move |_| pane.set(Some(ActivePane::Commit))>"Review and commit"</button>
                                    <button
                                        class="secondary-button"
                                        disabled=move || busy.get()
                                        title="Commit every change with an automatic message"
                                        on:click=move |_| {
                                            post_act(
                                                "/commits",
                                                json!({ "snapshot": true, "token": snap_token }),
                                                format!("Snapshot of {} committed.", plural(pending_count, "change", None)),
                                            );
                                        }
                                    >
                                        "Snapshot"
                                    </button>
                                </div>
                            })}
                        }.into_any()
                    } else {
                        view! { <p class="history-clean"><Icon name="check"/>"Everything is committed."</p> }.into_any()
                    }}
                </div>
                {(!branches.is_empty()).then(|| view! {
                    <div class="property-card">
                        <h3>"Agent branches"</h3>
                        {branches.into_iter().map(|br| {
                            let br_open = br.clone();
                            let count = br.changes.len();
                            view! {
                                <button class="property property-action" on:click=move |_| pane.set(Some(ActivePane::Branch(br_open.clone())))>
                                    <Icon name="agent"/>
                                    <div>
                                        <span>{br.message}</span>
                                        <small>{format!("{} \u{00b7} {}", plural(count, "file", None), when(br.modified_at))}</small>
                                    </div>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                })}
                <h3 class="history-heading">"History"</h3>
                {move || {
                    let mut all = initial_changesets.get_value();
                    all.extend(older.get());
                    if !all.is_empty() {
                        view! {
                            <ol class="history-log">
                                {all.into_iter().map(|row| {
                                    let icon = match row.kind.as_str() {
                                        "restore" => "history",
                                        "merge" => "agent",
                                        "snapshot" => "clock",
                                        _ => "check",
                                    };
                                    let row_open = row.clone();
                                    view! {
                                        <li>
                                            <button class="history-entry" on:click=move |_| pane.set(Some(ActivePane::Changeset(row_open.clone())))>
                                                <Icon name=icon/>
                                                <span>
                                                    <strong>{row.message}</strong>
                                                    <small>{format!("{} \u{00b7} {} \u{00b7} {}", row.author, when(row.created_at), plural(row.file_count, "file", None))}</small>
                                                </span>
                                            </button>
                                        </li>
                                    }
                                }).collect_view()}
                            </ol>
                        }.into_any()
                    } else {
                        view! { <p class="muted">"No commits yet. Commit the current files to start the history."</p> }.into_any()
                    }
                }}
                {move || next.get().is_some().then(|| view! {
                    <button class="text-button" on:click=load_more>"Show older"</button>
                })}
                <GitCard folder_id=fid refs=git_refs username=username/>
                <div class="history-footer">
                    {move || {
                        let has_any = !initial_changesets.with_value(Vec::is_empty) || !older.with(Vec::is_empty);
                        has_any.then(|| view! {
                            <a class="text-button" href=bundle_href.clone() download="">
                                <Icon name="download"/>
                                "Download as Git bundle"
                            </a>
                        })
                    }}
                    {owner.then(|| view! {
                        <button class="text-button danger" disabled=move || busy.get() on:click=move |_| untrack.set(true)>"Stop keeping history"</button>
                    })}
                </div>

                {move || match pane.get() {
                    Some(ActivePane::Commit) => Some(view! {
                        <CommitPane
                            folder_id=fid_sv.get_value()
                            status=status_sv.get_value()
                            session=session_sv.get_value()
                            on_close=Callback::new(move |_| pane.set(None))
                            on_done=done
                        />
                    }.into_any()),
                    Some(ActivePane::Changeset(row)) => Some(view! {
                        <ChangesetPane
                            folder_id=fid_sv.get_value()
                            changeset=row
                            revision=repo_rev
                            can_write=can_write
                            session=session_sv.get_value()
                            on_close=Callback::new(move |_| pane.set(None))
                            on_done=done
                        />
                    }.into_any()),
                    Some(ActivePane::Branch(br)) => Some(view! {
                        <BranchPane
                            folder_id=fid_sv.get_value()
                            branch=br
                            revision=repo_rev
                            can_write=can_write
                            session=session_sv.get_value()
                            on_close=Callback::new(move |_| pane.set(None))
                            on_done=done
                        />
                    }.into_any()),
                    None => None,
                }}

                {move || untrack.get().then(|| view! {
                    <Dialog title="Stop keeping history?" on_close=Callback::new(move |_| untrack.set(false))>
                        <p>{format!(
                            "All commits and agent branches for \u{201c}{}\u{201d} are deleted. Files and their versions stay; content only history kept is released from your storage.",
                            folder_name.get_value()
                        )}</p>
                        <div class="dialog-actions">
                            <button class="text-button" on:click=move |_| untrack.set(false)>"Cancel"</button>
                            <button
                                class="danger-button"
                                disabled=move || busy.get()
                                on:click=move |_| {
                                    untrack.set(false);
                                    post_act("", json!({ "action": "untrack", "revision": repo_rev }), "History is off for this folder.".to_owned());
                                }
                            >
                                "Delete history"
                            </button>
                        </div>
                    </Dialog>
                })}
            </div>
        }
        .into_any()
    }
}
