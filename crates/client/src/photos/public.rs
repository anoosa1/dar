//! A Photos shared link (`/s/<token>`) for visitors without an account (`photos/public.tsx`):
//! the Photos grid and viewer in a minimal frame, an optional password form, downloads and
//! uploads when allowed.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};

use super::{
    PhotoAsset, PhotoPage, PhotoPublicDetail, PhotoPublicShare, Playback, Transfer, css_escape,
    encode_uri_component, error_message, file_last_modified, format_duration_short, format_number,
    put_upload_file, request_animation_frame,
};
use crate::api::{self, Req};
use crate::dates::{local_date, local_date_time};
use crate::icons::Icon;
use crate::ui::{document, listen, window};

fn apply_system_theme() {
    let dark = window()
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches());
    if let Some(root) = document().document_element() {
        let _ = root.set_attribute("data-theme", if dark { "mocha" } else { "latte" });
    }
}

#[derive(Deserialize)]
struct UploadReservation {
    id: String,
}

#[component]
pub fn PublicPhotos() -> impl IntoView {
    apply_system_theme();
    if let Ok(Some(media)) = window().match_media("(prefers-color-scheme: dark)") {
        listen(&media, "change", false, |_| apply_system_theme());
    }

    let pathname = window().location().pathname().unwrap_or_default();
    let token = pathname.split('/').nth(2).unwrap_or_default().to_owned();
    let api_base = StoredValue::new(format!("/public/photos/{token}"));

    let share: RwSignal<Option<PhotoPublicShare>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let items: RwSignal<Vec<PhotoAsset>> = RwSignal::new(Vec::new());
    let cursor = RwSignal::new(String::new());
    let open: RwSignal<Option<String>> = RwSignal::new(None);
    let transfers: RwSignal<Vec<Transfer>> = RwSignal::new(Vec::new());
    let file_input = NodeRef::<html::Input>::new();
    let return_focus: StoredValue<Option<String>> = StoredValue::new(None);

    let load = move || {
        let api = api_base.get_value();
        spawn_local(async move {
            match api::get::<PhotoPublicShare>(&api).await {
                Ok(value) => {
                    let needs_password = value.needs_password;
                    share.try_set(Some(value));
                    error.try_set(String::new());
                    if !needs_password {
                        match api::get::<PhotoPage>(&format!("{api}/assets")).await {
                            Ok(page) => {
                                items.try_set(page.items);
                                cursor.try_set(page.cursor);
                            }
                            Err(cause) => {
                                error.try_set(error_message(&cause.message));
                            }
                        }
                    }
                }
                Err(cause) => {
                    let msg = if cause.status == 404 {
                        "This link is no longer available. It may have expired or been removed."
                            .to_owned()
                    } else {
                        error_message(&cause.message)
                    };
                    error.try_set(msg);
                }
            }
        });
    };
    load();

    Effect::new(move |_| {
        share.with(|s| {
            if let Some(s) = s
                && !s.needs_password
            {
                document().set_title(&format!("{} \u{00b7} Dar", s.title));
            }
        });
    });

    let more = move || {
        let after = cursor.get_untracked();
        if after.is_empty() {
            return;
        }
        let api = api_base.get_value();
        spawn_local(async move {
            let url = format!("{api}/assets?cursor={}", encode_uri_component(&after));
            if let Ok(page) = api::get::<PhotoPage>(&url).await {
                items.try_update(|current| current.extend(page.items));
                cursor.try_set(page.cursor);
            }
        });
    };

    let close = move || {
        return_focus.set_value(open.get_untracked());
        open.set(None);
    };

    Effect::new(move |_| {
        if open.get().is_none()
            && let Some(id) = return_focus.get_value()
        {
            return_focus.set_value(None);
            request_animation_frame(move || {
                let selector = format!("[data-asset=\"{}\"] .photos-open", css_escape(&id));
                if let Some(el) = document()
                    .query_selector(&selector)
                    .ok()
                    .flatten()
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = el.focus();
                }
            });
        }
    });

    listen(&document(), "keydown", false, move |event| {
        let Some(open_id) = open.get_untracked() else {
            return;
        };
        let event: web_sys::KeyboardEvent = event.unchecked_into();
        let in_field = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| t.closest("input,textarea,select").ok().flatten())
            .is_some();
        if event.default_prevented() || in_field {
            return;
        }
        let list = items.get_untracked();
        let Some(index) = list.iter().position(|i| i.id == open_id) else {
            return;
        };
        match event.key().as_str() {
            "ArrowRight" => {
                if let Some(next) = list.get(index + 1) {
                    event.prevent_default();
                    open.set(Some(next.id.clone()));
                }
            }
            "ArrowLeft" => {
                if index > 0
                    && let Some(prev) = list.get(index - 1)
                {
                    event.prevent_default();
                    open.set(Some(prev.id.clone()));
                }
            }
            "Escape" => {
                event.prevent_default();
                close();
            }
            _ => {}
        }
    });

    let upload = move |files: Vec<web_sys::File>| {
        let files = send_wrapper::SendWrapper::new(files);
        let api = api_base.get_value();
        spawn_local(async move {
            for file in files.take() {
                let id = crate::random_uuid();
                let name = file.name();
                transfers.try_update(|list| {
                    list.push(Transfer {
                        id: id.clone(),
                        name: name.clone(),
                        progress: 0.0,
                        status: "uploading",
                        message: None,
                    });
                });
                let update_transfer = move |tid: &str, f: &dyn Fn(&mut Transfer)| {
                    transfers.try_update(|list| {
                        if let Some(t) = list.iter_mut().find(|t| t.id == tid) {
                            f(t);
                        }
                    });
                };
                let mut upload_id = String::new();
                let when = js_sys::Date::new(&JsValue::from_f64(file_last_modified(&file)));
                let media_type = file.type_();
                let body = json!({
                    "name": name,
                    "size": file.size(),
                    "mediaType": if media_type.is_empty() { "application/octet-stream".to_owned() } else { media_type },
                    "year": when.get_full_year(),
                    "month": when.get_month() + 1,
                });
                let result: Result<(), String> = async {
                    let res: UploadReservation = api::request(
                        &format!("{api}/uploads"),
                        Req {
                            method: "POST",
                            body: Some(body.to_string()),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|e| error_message(&e.message))?;
                    upload_id = res.id;
                    let progress_id = id.clone();
                    put_upload_file(
                        &format!("{api}/uploads/{upload_id}"),
                        None,
                        &file,
                        move |p| {
                            update_transfer(&progress_id, &|t| t.progress = p);
                        },
                    )
                    .await?;
                    Ok(())
                }
                .await;
                match result {
                    Ok(()) => update_transfer(&id, &|t| {
                        t.progress = 1.0;
                        t.status = "complete";
                    }),
                    Err(msg) => {
                        update_transfer(&id, &|t| {
                            t.status = "error";
                            t.message = Some(msg.clone());
                        });
                        if !upload_id.is_empty() {
                            let _ = api::request::<Value>(
                                &format!("{api}/uploads/{upload_id}"),
                                Req {
                                    method: "DELETE",
                                    ..Default::default()
                                },
                            )
                            .await;
                        }
                    }
                }
            }
            load();
        });
    };

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum PageState {
        Error,
        Loading,
        Password,
        Ready,
    }

    let page_state = Memo::new(move |_| {
        if !error.get().is_empty() {
            PageState::Error
        } else {
            match share.with(|s| s.as_ref().map(|s| s.needs_password)) {
                None => PageState::Loading,
                Some(true) => PageState::Password,
                Some(false) => PageState::Ready,
            }
        }
    });

    let has_current = Memo::new(move |_| {
        let Some(id) = open.get() else { return false };
        items.with(|list| list.iter().any(|i| i.id == id))
    });

    let current_index = Memo::new(move |_| {
        let id = open.get()?;
        items.with(|list| list.iter().position(|i| i.id == id))
    });

    let grid_rows = Memo::new(move |_| {
        let show_meta = share.with(|s| s.as_ref().is_some_and(|s| s.show_metadata));
        let mut last_month = String::new();
        items.with(|list| {
            list.iter()
                .map(|item| {
                    let month =
                        local_date(item.taken_at, &[("month", "long"), ("year", "numeric")]);
                    let heading = if show_meta && month != last_month {
                        last_month = month.clone();
                        Some(month)
                    } else {
                        None
                    };
                    (heading, item.clone())
                })
                .collect::<Vec<_>>()
        })
    });

    view! {
        <div class="public-photos">
            <header class="public-photos-bar">
                <span class="brand"><span class="brand-mark" aria-hidden="true"></span>"Dar"</span>
            </header>
            {move || match page_state.get() {
                PageState::Error => view! {
                    <main class="photos-empty" role="alert">
                        <Icon name="link"/>
                        <h1>"Link unavailable"</h1>
                        <p>{move || error.get()}</p>
                    </main>
                }.into_any(),
                PageState::Loading => view! {
                    <main class="photos-empty" role="status" aria-busy="true">"Opening shared photos\u{2026}"</main>
                }.into_any(),
                PageState::Password => view! {
                    <PasswordForm api=api_base.get_value() on_unlocked=Callback::new(move |_| load())/>
                }.into_any(),
                PageState::Ready => view! {
                    <main
                        class="public-photos-main"
                        aria-label=move || share.with(|s| s.as_ref().map(|s| s.title.clone()).unwrap_or_default())
                    >
                        {move || if has_current.get() {
                            let share_sig = Signal::from(Memo::new(move |last| share.get().or_else(|| last.cloned()).expect("share ready")));
                            let asset_sig = Signal::derive(move || {
                                current_index.get().and_then(|idx| items.with(|l| l.get(idx).cloned()))
                            });
                            let index_sig = Signal::derive(move || current_index.get().unwrap_or(0));
                            let total_sig = Signal::derive(move || items.with(Vec::len));
                            view! {
                                <PublicViewer
                                    api=api_base.get_value()
                                    asset=asset_sig
                                    index=index_sig
                                    total=total_sig
                                    share=share_sig
                                    on_close=Callback::new(move |_| close())
                                    on_step=Callback::new(move |delta: isize| {
                                        let Some(idx) = current_index.get_untracked() else { return };
                                        let next_idx = idx as isize + delta;
                                        let list = items.get_untracked();
                                        if next_idx >= 0
                                            && let Some(next) = list.get(next_idx as usize)
                                        {
                                            open.set(Some(next.id.clone()));
                                        }
                                        if next_idx >= list.len() as isize - 4 {
                                            more();
                                        }
                                    })
                                />
                            }.into_any()
                        } else {
                            view! {
                                <div class="public-photos-heading">
                                    <div>
                                        <h1>{move || share.with(|s| s.as_ref().map(|s| s.title.clone()).unwrap_or_default())}</h1>
                                        <p>{move || share.with(|s| {
                                            let Some(s) = s else { return String::new() };
                                            let by = if !s.owner.is_empty() {
                                                format!("Shared by {}", s.owner)
                                            } else {
                                                "Shared photos".to_owned()
                                            };
                                            let exp = if s.expires_at > 0.0 {
                                                format!(" \u{00b7} until {}", local_date(s.expires_at, &[("dateStyle", "medium")]))
                                            } else {
                                                String::new()
                                            };
                                            format!("{by} \u{00b7} {} item{}{exp}", format_number(s.count), if s.count == 1 { "" } else { "s" })
                                        })}</p>
                                        {move || share.with(|s| {
                                            let desc = s.as_ref().map(|s| s.description.clone()).unwrap_or_default();
                                            (!desc.is_empty()).then(|| view! { <p class="public-photos-description">{desc}</p> })
                                        })}
                                    </div>
                                    {move || share.with(|s| s.as_ref().is_some_and(|s| s.allow_upload)).then(|| view! {
                                        <button type="button" class="primary-button" on:click=move |_| {
                                            if let Some(input) = file_input.get_untracked() {
                                                input.click();
                                            }
                                        }>
                                            <Icon name="upload"/>"Add photos"
                                        </button>
                                        <input
                                            node_ref=file_input
                                            type="file"
                                            multiple
                                            hidden
                                            accept="image/*,video/*,.heic,.heif,.dng,.mov"
                                            aria-label="Add photos to this album"
                                            on:change=move |e| {
                                                if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                                    let files: Vec<web_sys::File> = input
                                                        .files()
                                                        .map(|list| (0..list.length()).filter_map(|i| list.item(i)).collect())
                                                        .unwrap_or_default();
                                                    input.set_value("");
                                                    if !files.is_empty() {
                                                        upload(files);
                                                    }
                                                }
                                            }
                                        />
                                    })}
                                </div>
                                {move || items.with(Vec::is_empty).then(|| view! {
                                    <div class="photos-empty">
                                        <Icon name="image"/>
                                        <h2>"Nothing here yet"</h2>
                                        {move || share.with(|s| s.as_ref().is_some_and(|s| s.allow_upload)).then(|| view! {
                                            <p>"Add the first photos to this album."</p>
                                        })}
                                    </div>
                                })}
                                <div
                                    class="photos-grid"
                                    role="list"
                                    aria-label=move || share.with(|s| format!("{} photos", s.as_ref().map(|s| s.title.as_str()).unwrap_or_default()))
                                    data-columns="5"
                                >
                                    <For
                                        each=move || grid_rows.get()
                                        key=|(heading, item)| (heading.clone(), item.id.clone(), item.revision)
                                        children=move |(heading, item)| {
                                            let api = api_base.get_value();
                                            let id = item.id.clone();
                                            let id_click = item.id.clone();
                                            let label = format!(
                                                "{} {}",
                                                if item.kind == "video" { "Video" } else { "Photo" },
                                                item.name
                                            );
                                            let is_video = item.kind == "video";
                                            let dur = if item.duration > 0.0 { format_duration_short(item.duration) } else { String::new() };
                                            let is_live = item.live.as_deref().is_some_and(|l| !l.is_empty());
                                            view! {
                                                {heading.map(|h| view! { <h2 class="photos-month">{h}</h2> })}
                                                <div role="listitem" data-asset=id class="photos-tile">
                                                    <button
                                                        type="button"
                                                        class="photos-open"
                                                        aria-label=label
                                                        on:click=move |_| open.set(Some(id_click.clone()))
                                                    >
                                                        <PublicThumb api=api asset=item size="s"/>
                                                        {is_video.then(|| view! { <span class="photos-badge"><Icon name="video"/>{dur}</span> })}
                                                        {is_live.then(|| view! { <span class="photos-badge"><Icon name="live"/>"Live"</span> })}
                                                    </button>
                                                </div>
                                            }
                                        }
                                    />
                                </div>
                                {move || (!cursor.get().is_empty()).then(|| view! {
                                    <div class="photos-more">
                                        <button class="text-button" on:click=move |_| more()>"Load more"</button>
                                    </div>
                                })}
                            }.into_any()
                        }}
                        {move || (!transfers.with(Vec::is_empty)).then(|| view! {
                            <section class="transfer-panel photos-transfers" aria-label="Uploads">
                                <div class="transfer-heading">
                                    <strong>"Uploads"</strong>
                                    <button
                                        class="icon-button"
                                        aria-label="Dismiss finished uploads"
                                        disabled=move || transfers.with(|t| t.iter().any(|x| x.status == "uploading"))
                                        on:click=move |_| transfers.set(Vec::new())
                                    >
                                        <Icon name="close"/>
                                    </button>
                                </div>
                                <ul>
                                    {move || transfers.get().into_iter().map(|t| {
                                        let cls = format!("photos-transfer is-{}", t.status);
                                        let label = format!("{} upload progress", t.name);
                                        let pct = format!("{}%", (t.progress * 100.0).round() as u32);
                                        view! {
                                            <li class=cls>
                                                <span>{t.name}</span>
                                                <progress max="1" value=t.progress.to_string() aria-label=label/>
                                                {match t.message {
                                                    Some(msg) => view! { <small role="alert">{msg}</small> }.into_any(),
                                                    None => view! { <small>{if t.status == "complete" { "Done".to_owned() } else { pct }}</small> }.into_any(),
                                                }}
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            </section>
                        })}
                    </main>
                }.into_any(),
            }}
        </div>
    }
}

#[component]
fn PasswordForm(api: String, on_unlocked: Callback<()>) -> impl IntoView {
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let input = NodeRef::<html::Input>::new();
    let api = StoredValue::new(api);

    Effect::new(move |_| {
        if let Some(el) = input.get() {
            let _ = el.focus();
        }
    });

    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        busy.set(true);
        error.set(String::new());
        let pw = password.get_untracked();
        let url = format!("{}/password", api.get_value());
        spawn_local(async move {
            let body = json!({ "password": pw }).to_string();
            match api::request::<Value>(
                &url,
                Req {
                    method: "POST",
                    body: Some(body),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    on_unlocked.try_run(());
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                    busy.try_set(false);
                }
            }
        });
    };

    view! {
        <main class="public-photos-main">
            <form class="locked-unlock" on:submit=submit>
                <span class="locked-unlock-icon" aria-hidden="true"><Icon name="lock"/></span>
                <h1>"Password required"</h1>
                <p>"Enter the password you were given to see these photos."</p>
                <label>"Password"
                    <input
                        node_ref=input
                        type="password"
                        autofocus
                        autocomplete="off"
                        prop:value=move || password.get()
                        on:input=move |e| {
                            password.set(event_target_value(&e));
                            error.set(String::new());
                        }
                        aria-invalid=move || (!error.get().is_empty()).to_string()
                    />
                </label>
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <button class="primary-button" disabled=move || busy.get() || password.with(String::is_empty)>
                    {move || if busy.get() { "Checking\u{2026}" } else { "Open" }}
                </button>
            </form>
        </main>
    }
}

#[component]
fn PublicThumb(api: String, asset: PhotoAsset, size: &'static str) -> impl IntoView {
    let failed = RwSignal::new(false);
    let src = format!(
        "{api}/assets/{}/thumbnail?{}v={}",
        asset.id,
        if size == "m" { "size=m&" } else { "" },
        asset.revision
    );
    let alt = if size == "m" {
        asset.name.clone()
    } else {
        String::new()
    };
    let icon = if asset.kind == "video" {
        "video"
    } else {
        "image"
    };
    let name = asset.name;
    move || {
        if failed.get() {
            view! {
                <span class="photos-placeholder"><Icon name=icon/><small>{name.clone()}</small></span>
            }
            .into_any()
        } else {
            view! {
                <img
                    src=src.clone()
                    alt=alt.clone()
                    loading="lazy"
                    decoding="async"
                    draggable="false"
                    on:error=move |_| failed.set(true)
                />
            }
            .into_any()
        }
    }
}

#[component]
fn PublicViewer(
    api: String,
    asset: Signal<Option<PhotoAsset>>,
    index: Signal<usize>,
    total: Signal<usize>,
    share: Signal<PhotoPublicShare>,
    on_close: Callback<()>,
    on_step: Callback<isize>,
) -> impl IntoView {
    let detail: RwSignal<Option<PhotoPublicDetail>> = RwSignal::new(None);
    let heading = NodeRef::<html::H1>::new();
    let api = StoredValue::new(api);
    let seq = RwSignal::new(0u32);

    Effect::new(move |_| {
        if let Some(el) = heading.get() {
            let _ = el.focus();
        }
    });

    let asset_id =
        Memo::new(move |_| asset.with(|a| a.as_ref().map(|a| a.id.clone()).unwrap_or_default()));

    Effect::new(move |_| {
        let id = asset_id.get();
        detail.set(None);
        if id.is_empty() || !share.with_untracked(|s| s.show_metadata) {
            return;
        }
        let ticket = seq.get_untracked() + 1;
        seq.set(ticket);
        let url = format!("{}/assets/{id}", api.get_value());
        spawn_local(async move {
            if let Ok(val) = api::get::<PhotoPublicDetail>(&url).await
                && seq.try_get_untracked() == Some(ticket)
            {
                detail.try_set(Some(val));
            }
        });
    });

    let meta_text = move || {
        detail.with(|d| {
            let d = d.as_ref()?;
            let camera = d
                .exif
                .as_ref()
                .map(|e| {
                    let mut parts = Vec::new();
                    if let (Some(make), Some(model)) = (&e.make, &e.model)
                        && !make.is_empty()
                        && !model.is_empty()
                        && !model.starts_with(make.as_str())
                    {
                        parts.push(make.clone());
                    }
                    if let Some(model) = &e.model
                        && !model.is_empty()
                    {
                        parts.push(model.clone());
                    }
                    parts.join(" ")
                })
                .unwrap_or_default();
            let mut bits = Vec::new();
            if let Some(taken) = d.taken_at
                && taken > 0.0
            {
                bits.push(local_date_time(
                    taken,
                    &[("dateStyle", "long"), ("timeStyle", "short")],
                ));
            }
            if !camera.is_empty() {
                bits.push(camera);
            }
            if let Some(desc) = &d.description
                && !desc.is_empty()
            {
                bits.push(desc.clone());
            }
            if bits.is_empty() {
                None
            } else {
                Some(bits.join(" \u{00b7} "))
            }
        })
    };

    view! {
        <section
            class="photos-viewer public-photos-viewer"
            aria-label=move || format!("Viewer for {}", asset.with(|a| a.as_ref().map(|a| a.name.clone()).unwrap_or_default()))
        >
            <div class="photos-viewer-bar">
                <button type="button" class="icon-button" aria-label="Back to photos" title="Back (Escape)" on:click=move |_| { on_close.try_run(()); }>
                    <Icon name="back"/>
                </button>
                <h1 node_ref=heading tabindex="-1">
                    {move || asset.with(|a| a.as_ref().map(|a| a.name.clone()).unwrap_or_default())}
                </h1>
                <span class="photos-heading-count">{move || format!("{} of {}", index.get() + 1, total.get())}</span>
                <span class="photos-spacer"></span>
                <button type="button" class="icon-button" aria-label="Previous" disabled=move || index.get() == 0 on:click=move |_| { on_step.try_run(-1); }>
                    <Icon name="back"/>
                </button>
                <button type="button" class="icon-button" aria-label="Next" disabled={move || index.get() + 1 >= total.get()} on:click=move |_| { on_step.try_run(1); }>
                    <Icon name="chevron"/>
                </button>
                {move || share.with(|s| s.allow_download).then(|| {
                    let href = format!("{}/assets/{}/original", api.get_value(), asset_id.get());
                    view! {
                        <a class="icon-button" href=href download aria-label="Download original" title="Download original">
                            <Icon name="download"/>
                        </a>
                    }
                })}
            </div>
            <div class="photos-stage">
                {move || {
                    let _ = asset_id.get();
                    let Some(current) = asset.get_untracked() else { return view! { <span></span> }.into_any() };
                    if current.kind == "video" {
                        view! { <PublicVideo api=api.get_value() asset=current/> }.into_any()
                    } else {
                        view! { <PublicThumb api=api.get_value() asset=current size="m"/> }.into_any()
                    }
                }}
            </div>
            {move || meta_text().map(|text| view! { <p class="public-photos-meta">{text}</p> })}
        </section>
    }
}

#[component]
fn PublicVideo(api: String, asset: PhotoAsset) -> impl IntoView {
    let state: RwSignal<Option<Playback>> = RwSignal::new(None);
    let poll_id = RwSignal::new(0u32);
    let name = asset.name.clone();
    let url = format!("{api}/assets/{}/playback", asset.id);

    {
        let id = poll_id.get_untracked() + 1;
        poll_id.set(id);
        spawn_local(async move {
            loop {
                if poll_id.try_get_untracked() != Some(id) {
                    return;
                }
                match api::request::<Playback>(
                    &url,
                    Req {
                        method: "POST",
                        ..Default::default()
                    },
                )
                .await
                {
                    Ok(val) => {
                        let preparing = val.state == "preparing";
                        state.try_set(Some(val));
                        if !preparing {
                            return;
                        }
                        crate::sleep(1500).await;
                    }
                    Err(_) => {
                        state.try_set(Some(Playback {
                            state: "failed".to_owned(),
                            progress: 0.0,
                            url: String::new(),
                        }));
                        return;
                    }
                }
            }
        });
    }

    move || {
        match state.get() {
        None => view! {
            <div class="photos-video-wait" role="status">
                <span class="photos-spinner" aria-hidden="true"></span>
                <span>"Preparing this video\u{2026}"</span>
            </div>
        }
        .into_any(),
        Some(pb) if pb.state == "preparing" => {
            let suffix = if pb.progress > 0.0 {
                format!(" \u{00b7} {}%", (pb.progress * 100.0).round() as u32)
            } else {
                "\u{2026}".to_owned()
            };
            view! {
                <div class="photos-video-wait" role="status">
                    <span class="photos-spinner" aria-hidden="true"></span>
                    <span>"Preparing this video"{suffix}</span>
                </div>
            }
            .into_any()
        }
        Some(pb) if pb.state == "failed" => view! {
            <div class="photos-empty" role="alert">
                <Icon name="video"/>
                <h2>"This video can\u{2019}t be played here"</h2>
            </div>
        }
        .into_any(),
        Some(pb) => view! {
            <video src=pb.url controls playsinline preload="metadata" aria-label=name.clone()></video>
        }
        .into_any(),
    }
    }
}
