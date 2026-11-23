//! The Files text editor (`text-editor.tsx`): autosave, a recovery draft in this tab, and a
//! comparison when the saved file changed underneath.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use super::types::{file_type_label, FileTile};
use super::FileEntry;
use crate::api::{self, BrowserSession, Req, RequestError};
use crate::dates::edited;
use crate::icons::Icon;
use crate::markdown::{format_markdown, Markdown, MarkdownToolbar};
use crate::ui::{listen, window, ActionMenu, MenuAction};

pub fn editable_text(entry: &FileEntry) -> bool {
    let name = entry.name.to_lowercase();
    entry.kind == "file" && entry.size <= 1024.0 * 1024.0 && (name.ends_with(".txt") || name.ends_with(".md") || name.ends_with(".markdown") || entry.media_type.starts_with("text/") || entry.media_type == "application/json")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Upload {
    id: String,
    file_id: String,
}

pub struct TextFile<'a> {
    pub name: &'a str,
    pub parent_id: &'a str,
    pub content: &'a str,
    pub media_type: &'a str,
    pub file_id: Option<&'a str>,
    pub expected_revision: Option<u32>,
}

/// Reserves an upload and sends the text; returns the file's id.
pub async fn write_text_file(session: &BrowserSession, file: TextFile<'_>) -> Result<String, RequestError> {
    let bytes = file.content.as_bytes();
    if bytes.len() > 1024 * 1024 {
        return Err(RequestError { status: 413, message: "Text editing is limited to 1 MiB.".into() });
    }
    let mut body = serde_json::json!({ "name": file.name, "parentId": file.parent_id, "mediaType": file.media_type, "size": bytes.len() });
    if let (Some(id), Some(revision)) = (file.file_id, file.expected_revision) {
        body["fileId"] = id.into();
        body["expectedRevision"] = revision.into();
    }
    send_upload(session, body, js_sys::Uint8Array::from(bytes).into()).await
}

/// Uploads a browser file (pasted or dropped) as a new file in `parent_id`; returns its id.
pub async fn upload_blob(session: &BrowserSession, name: &str, parent_id: &str, media_type: &str, blob: &web_sys::Blob) -> Result<String, RequestError> {
    let body = serde_json::json!({ "name": name, "parentId": parent_id, "mediaType": media_type, "size": blob.size() as u64 });
    send_upload(session, body, blob.into()).await
}

/// Reserves an upload with `body`, then sends `raw`; a failed transfer cancels the reservation.
async fn send_upload(session: &BrowserSession, body: serde_json::Value, raw: wasm_bindgen::JsValue) -> Result<String, RequestError> {
    let upload: Upload = api::send("/api/v1/files/uploads", "POST", Some(body), &session.csrf_token).await?;
    let result = api::fetch(&format!("/api/v1/files/uploads/{}", upload.id), Req { method: "PUT", raw: Some(raw), csrf: Some(&session.csrf_token), ..Default::default() }).await;
    let failure = match result {
        Ok(response) if response.ok() => return Ok(upload.file_id),
        Ok(response) => {
            let status = response.status();
            let message = match response.json() {
                Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await.ok().and_then(|v| js_sys::Reflect::get(&v, &"error".into()).ok()).and_then(|e| js_sys::Reflect::get(&e, &"message".into()).ok()).and_then(|m| m.as_string()),
                Err(_) => None,
            };
            RequestError { status, message: message.unwrap_or_else(|| "Save failed.".into()) }
        }
        Err(e) => e,
    };
    let _ = api::request::<serde_json::Value>(&format!("/api/v1/files/uploads/{}", upload.id), Req { method: "DELETE", csrf: Some(&session.csrf_token), ..Default::default() }).await;
    Err(failure)
}

async fn read_content(id: &str) -> Result<String, String> {
    let response = api::fetch(&format!("/api/v1/files/{id}/content"), Default::default()).await.map_err(|e| e.message)?;
    if !response.ok() {
        return Err("The file could not be loaded.".into());
    }
    let text = wasm_bindgen_futures::JsFuture::from(response.text().map_err(|_| "The file could not be loaded.")?).await.map_err(|_| "The file could not be loaded.")?;
    Ok(text.as_string().unwrap_or_default())
}

fn session_storage() -> Option<web_sys::Storage> {
    window().session_storage().ok().flatten()
}

#[component]
pub fn TextEditor(entry: FileEntry, session: BrowserSession, on_close: Callback<()>) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let saved = RwSignal::new(String::new());
    let status = RwSignal::new("Loading…");
    let error = RwSignal::new(String::new());
    let ready = RwSignal::new(false);
    let read_only = RwSignal::new(true);
    let preview = RwSignal::new(false);
    let discard = RwSignal::new(false);
    let recovery: RwSignal<Option<String>> = RwSignal::new(None);
    let conflict: RwSignal<Option<(String, FileEntry)>> = RwSignal::new(None);
    let modified = RwSignal::new(entry.modified_at);
    let revision = RwSignal::new(entry.revision);
    let saving = RwSignal::new(false);
    let history: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let future: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let input = NodeRef::<html::Textarea>::new();
    let key = format!("dar.file-draft.{}.{}", session.user.id, entry.id);
    let markdown = { let n = entry.name.to_lowercase(); n.ends_with(".md") || n.ends_with(".markdown") };
    let dirty = Memo::new(move |_| text.with(|t| saved.with(|s| t != s)));
    let entry = StoredValue::new(entry);
    let session = StoredValue::new(session);
    let key = StoredValue::new(key);

    // The details give access and the current revision.
    {
        spawn_local(async move {
            let detail = match super::file_details(&entry.with_value(|e| e.id.clone())).await {
                Ok(d) => d,
                Err(e) => return error.set(e),
            };
            if !editable_text(&detail.entry) || detail.entry.trashed {
                return error.set("This file cannot be edited as text.".into());
            }
            match read_content(&detail.entry.id).await {
                Ok(content) => {
                    revision.set(detail.entry.revision);
                    modified.set(detail.entry.modified_at);
                    text.set(content.clone());
                    saved.set(content.clone());
                    let read = detail.role == "read";
                    read_only.set(read);
                    ready.set(true);
                    status.set(if read { "Read only" } else { "Saved" });
                    if let Some(draft) = session_storage().and_then(|s| s.get_item(&key.get_value()).ok().flatten()).filter(|d| *d != content) {
                        recovery.set(Some(draft));
                    }
                }
                Err(e) => error.set(e),
            }
        });
    }
    // The recovery draft follows the text; leaving with unsaved changes asks first.
    Effect::new(move |_| {
        let (t, d) = (text.get(), dirty.get());
        if !ready.get() || read_only.get() || recovery.with(Option::is_some) {
            return;
        }
        if let Some(s) = session_storage() {
            let result = if d { s.set_item(&key.get_value(), &t) } else { s.remove_item(&key.get_value()) };
            if result.is_err() {
                error.set("Recovery storage is unavailable. Keep this editor open until the file is saved.".into());
            }
        }
    });
    listen(&window(), "beforeunload", false, move |event| if dirty.get_untracked() { event.prevent_default() });

    let compare = move || {
        spawn_local(async move {
            let id = entry.with_value(|e| e.id.clone());
            let result = async {
                let response = super::file_details(&id).await?;
                let content = read_content(&id).await.map_err(|_| "Saved version unavailable.".to_owned())?;
                Ok::<_, String>((content, response.entry))
            };
            match result.await {
                Ok(value) => conflict.set(Some(value)),
                Err(e) => error.set(e),
            }
        });
    };
    let save = move || {
        if !ready.get_untracked() || read_only.get_untracked() || saving.get_untracked() || recovery.with_untracked(Option::is_some) || conflict.with_untracked(Option::is_some) || !dirty.get_untracked() {
            return;
        }
        let content = text.get_untracked();
        let expected = revision.get_untracked();
        saving.set(true);
        status.set("Saving…");
        error.set(String::new());
        spawn_local(async move {
            let (entry, session) = (entry.get_value(), session.get_value());
            let result = write_text_file(&session, TextFile { name: &entry.name, parent_id: &entry.parent_id, content: &content, media_type: &entry.media_type, file_id: Some(&entry.id), expected_revision: Some(expected) }).await;
            match result {
                Ok(_) => {
                    revision.set(expected + 1);
                    saved.set(content);
                    modified.set(js_sys::Date::now());
                    status.set("Saved");
                }
                Err(e) => {
                    error.set(e.message.clone());
                    status.set("Save failed");
                    if [409, 412].contains(&e.status) {
                        compare();
                    }
                }
            }
            saving.set(false);
        });
    };
    // Autosave 900 ms after typing stops.
    let typed = RwSignal::new(0u32);
    Effect::new(move |_| {
        text.track();
        saved.track();
        if !dirty.get() || !error.with(String::is_empty) || !ready.get() || recovery.with(Option::is_some) || conflict.with(Option::is_some) {
            return;
        }
        let id = typed.get_untracked() + 1;
        typed.set(id);
        spawn_local(async move {
            crate::sleep(900).await;
            if typed.try_get_untracked() == Some(id) {
                save();
            }
        });
    });
    Effect::new(move |_| {
        if ready.get() && !read_only.get() && !preview.get()
            && let Some(i) = input.get() {
                let options = web_sys::FocusOptions::new();
                options.set_prevent_scroll(true);
                let _ = i.focus_with_options(&options);
            }
    });
    let change = move |value: String| {
        history.update(|h| {
            h.push(text.get_untracked());
            if h.len() > 100 {
                h.remove(0);
            }
        });
        future.set(Vec::new());
        text.set(value);
        status.set("Unsaved");
    };
    let format = Callback::new(move |(before, after): (&'static str, &'static str)| {
        let Some(field) = input.get_untracked() else { return };
        let (start, end) = (field.selection_start().ok().flatten().unwrap_or(0), field.selection_end().ok().flatten().unwrap_or(0));
        let (next, s, e) = format_markdown(&text.get_untracked(), start, end, before, after);
        change(next);
        let field = send_wrapper::SendWrapper::new(field);
        request_animation_frame(move || {
            let _ = field.focus();
            let _ = field.set_selection_range(s, e);
        });
    });
    let phase = move || {
        if read_only.get() {
            "read"
        } else if matches!(status.get(), "Saving…" | "Loading…") {
            "saving"
        } else if dirty.get() {
            if status.get() == "Save failed" { "error" } else { "dirty" }
        } else {
            "saved"
        }
    };
    let label = move || {
        let s = status.get();
        if matches!(s, "Loading…" | "Saving…") || read_only.get() {
            s
        } else if dirty.get() {
            if s == "Save failed" { "Not saved" } else { "Unsaved changes" }
        } else {
            "All changes saved"
        }
    };
    let close = move || if dirty.get_untracked() || saving.get_untracked() { discard.set(true) } else { { on_close.try_run(()); } };
    let id = entry.with_value(|e| e.id.clone());
    let menu = vec![MenuAction::new("Download", "download").href(format!("/api/v1/files/{id}/content")).download()];
    let menu = StoredValue::new(menu);
    let (kind, name, media) = entry.with_value(|e| (e.kind.clone(), e.name.clone(), e.media_type.clone()));
    let title = if markdown { name.trim_end_matches(".markdown").trim_end_matches(".md").to_owned() } else { name.clone() };
    let title = if markdown { let lower = name.to_lowercase(); if lower.ends_with(".markdown") { name[..name.len() - 9].to_owned() } else if lower.ends_with(".md") { name[..name.len() - 3].to_owned() } else { title } } else { title };
    view! {
        <section class=if markdown { "file-text-editor document-editor" } else { "file-text-editor document-editor is-plain-text" } aria-label=format!("Editor for {name}")
            on:keydown=move |e| {
                if (e.ctrl_key() || e.meta_key()) && e.key().to_lowercase() == "s" { e.prevent_default(); save(); }
                else if e.key() == "Escape" && !e.default_prevented() { e.prevent_default(); close(); }
            }>
            <div class="workspace-editor-top">
                <button class="icon-button" aria-label="Close editor" title="Close editor" on:click=move |_| close()><Icon name="close"/></button>
                <span class="note-notebook"><FileTile kind=kind.clone() name=name.clone() media_type=media.clone()/>{file_type_label(&kind, &name, &media)}</span>
                <span class=move || format!("note-status note-status-{}", phase()) role="status">{move || (phase() == "saved").then(|| view! { <Icon name="check"/> })}{label}</span>
                {move || (dirty.get() && !read_only.get()).then(|| view! {
                    <button class="text-button" disabled=move || status.get() == "Saving…" || conflict.with(Option::is_some) || recovery.with(Option::is_some) on:click=move |_| save()>"Save now"</button>
                })}
                {markdown.then(|| view! {
                    <div class="segmented" role="group" aria-label="Editor mode">
                        <button aria-pressed=move || (!preview.get()).to_string() on:click=move |_| preview.set(false)>"Write"</button>
                        <button aria-pressed=move || preview.get().to_string() on:click=move |_| preview.set(true)>"Preview"</button>
                    </div>
                })}
                <ActionMenu label="File options" actions=move |_| menu.get_value()><Icon name="more"/></ActionMenu>
            </div>
            {move || discard.get().then(|| view! {
                <div class="draft-banner" role="alert"><p>{if saving.get() { "A save is in progress. Keep the editor open until it completes." } else { "Unsaved changes are retained as a recovery draft in this tab." }}</p>
                    <button class="text-button" on:click=move |_| discard.set(false)>"Keep editing"</button>
                    <button class="text-button danger" disabled=move || saving.get() on:click=move |_| { on_close.try_run(()); }>"Close with recovery draft"</button></div>
            })}
            {move || (!error.get().is_empty()).then(|| view! {
                <div class="error-banner" role="alert"><span>{error.get()}</span>
                    <button class="text-button" on:click=move |_| save()>"Retry save"</button>
                    <button class="text-button" on:click=move |_| compare()>"Review saved version"</button></div>
            })}
            {move || recovery.get().map(|draft| view! {
                <div class="draft-banner" role="alert"><p>"A recovery draft is available."</p>
                    <button class="text-button" on:click=move |_| { change(draft.clone()); recovery.set(None); }>"Restore draft"</button>
                    <button class="text-button" on:click=move |_| { recovery.set(None); if let Some(s) = session_storage() { let _ = s.remove_item(&key.get_value()); } }>"Discard draft"</button></div>
            })}
            {move || conflict.get().map(|(content, latest)| {
                let (content2, latest2) = (content.clone(), latest.clone());
                view! {
                    <div class="draft-banner" role="alert"><p>"The saved file changed. Your text has been kept."</p>
                        <details><summary>"View saved version"</summary><pre>{content.clone()}</pre></details>
                        <button class="text-button" on:click=move |_| { revision.set(latest2.revision); modified.set(latest2.modified_at); text.set(content2.clone()); saved.set(content2.clone()); conflict.set(None); error.set(String::new()); status.set("Saved"); }>"Use saved version"</button>
                        <button class="text-button" on:click=move |_| { revision.set(latest.revision); conflict.set(None); error.set(String::new()); status.set("Unsaved"); }>"Keep my version"</button></div>
                }
            })}
            <div class="note-document">
                <h1 class="note-title">{title}</h1>
                <div class="note-meta"><span>"Edited "{move || edited(modified.get())}</span></div>
                {move || (markdown && !preview.get()).then(|| view! {
                    <MarkdownToolbar on_format=format disabled=Signal::derive(move || read_only.get() || !ready.get())>
                        <button type="button" class="text-button starts-group" aria-label="Undo" title="Undo" disabled=move || history.with(Vec::is_empty) || read_only.get()
                            on:click=move |_| { if let Some(previous) = history.try_update(|h| h.pop()).flatten() { future.update(|f| f.push(text.get_untracked())); text.set(previous); } }><Icon name="undo"/></button>
                        <button type="button" class="text-button" aria-label="Redo" title="Redo" disabled=move || future.with(Vec::is_empty) || read_only.get()
                            on:click=move |_| { if let Some(next) = future.try_update(|f| f.pop()).flatten() { history.update(|h| h.push(text.get_untracked())); text.set(next); } }><Icon name="redo"/></button>
                    </MarkdownToolbar>
                })}
                {move || if preview.get() {
                    view! { <Markdown source=text.get()/> }.into_any()
                } else {
                    view! {
                        <textarea node_ref=input class="note-source" aria-label="File contents" spellcheck=markdown.to_string() placeholder=move || ready.get().then_some("Start writing…")
                            readonly=move || read_only.get() || !ready.get() || recovery.with(Option::is_some) prop:value=move || text.get()
                            on:input=move |e| change(event_target_value(&e))></textarea>
                    }.into_any()
                }}
            </div>
        </section>
    }
}

#[allow(dead_code)]
fn element(target: Option<web_sys::EventTarget>) -> Option<web_sys::Element> {
    target.and_then(|t| t.dyn_into().ok())
}
