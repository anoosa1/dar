//! Git creation pane (`NewRepository`), cleanly separated from workspace orchestration.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use super::model::{HistoryOverview, is_valid_folder_name};
use super::parts::GitSegmented;
use crate::api::{self, BrowserSession};
use crate::files::picker::{FolderPicker, Selection};
use crate::live;
use crate::module_bindings::create_folder_reducer::create_folder;
use crate::random_uuid;
use crate::ui::{EditorPane, unique_id, window};

#[derive(Deserialize)]
struct UploadReservation {
    id: String,
}

async fn track_folder(folder_id: &str, csrf: &str) -> Result<(), String> {
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    let headers = web_sys::Headers::new()
        .map_err(|_| "History could not be turned on for this folder.".to_owned())?;
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
        Ok(fut) => fut
            .await
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default(),
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

/// Starts a new Files folder with history (and optional README.md) or tracks an existing folder.
#[component]
pub fn NewRepository(
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
                    let put_resp: web_sys::Response =
                        JsFuture::from(window().fetch_with_str_and_init(&put_url, &init))
                            .await
                            .and_then(|v| v.dyn_into::<web_sys::Response>())
                            .map_err(|_| "The README could not be added.".to_owned())?;
                    if !put_resp.ok() {
                        return Err("The README could not be added.".to_owned());
                    }

                    let status =
                        api::get::<HistoryOverview>(&format!("/api/v1/files/history/{id}"))
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
                Ok(id) => {
                    on_created.try_run(id);
                }
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
                                "Creating\u{2026}".to_owned()
                            } else if mode.get() == "new" {
                                "Create".to_owned()
                            } else {
                                "Keep history".to_owned()
                            }
                        }}
                    </button>
                </div>
            </form>
        </EditorPane>
    }
}
