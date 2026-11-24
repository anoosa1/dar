//! A file preview in the right-hand sheet (`file-preview.tsx`); PDFs use the Books viewer.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use super::FileEntry;
use super::types::{FileTile, file_type_label};
use crate::api;
use crate::dates::edited;
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction, active_element};

/// What the browser can show from the inline content route (`?preview=1`).
pub fn preview_kind(entry: &FileEntry) -> Option<&'static str> {
    if entry.kind != "file" {
        return None;
    }
    let media = entry.media_type.as_str();
    match media {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif" => Some("image"),
        "application/pdf" => Some("pdf"),
        "video/mp4" | "video/webm" | "video/ogg" => Some("video"),
        "audio/mpeg" | "audio/mp4" | "audio/ogg" | "audio/wav" | "audio/webm" | "audio/flac"
        | "audio/aac" | "audio/x-wav" | "audio/x-flac" => Some("audio"),
        _ if (media.starts_with("text/") || media == "application/json")
            && entry.size <= 1024.0 * 1024.0 =>
        {
            Some("text")
        }
        _ => None,
    }
}

fn format_size(bytes: f64) -> String {
    let (kb, mb, gb) = (1024.0, 1024.0 * 1024.0, 1024.0 * 1024.0 * 1024.0);
    if bytes < kb {
        format!("{bytes} B")
    } else if bytes < mb {
        format!("{:.1} KB", bytes / kb)
    } else if bytes < gb {
        format!("{:.1} MB", bytes / mb)
    } else {
        format!("{:.1} GB", bytes / gb)
    }
}

const ZOOM_STEPS: [u16; 6] = [0, 50, 75, 100, 150, 200];

fn zoom_class(level: u16) -> &'static str {
    match level {
        50 => "zoom-50",
        75 => "zoom-75",
        100 => "zoom-100",
        150 => "zoom-150",
        200 => "zoom-200",
        _ => "zoom-fit",
    }
}

#[component]
pub fn FilePreview(
    entry: FileEntry,
    siblings: Vec<FileEntry>,
    on_open: Callback<FileEntry>,
    on_close: Callback<()>,
    actions: Vec<MenuAction>,
    #[prop(optional)] on_share: Option<Callback<FileEntry>>,
    #[prop(optional)] on_toggle_details: Option<Callback<FileEntry>>,
    #[prop(optional, into)] details_open: Signal<bool>,
) -> impl IntoView {
    let kind = preview_kind(&entry);
    let content = format!("/api/v1/files/{}/content", entry.id);
    let source = format!("{content}?preview=1&v={}", entry.revision);
    let text: RwSignal<Option<String>> = RwSignal::new(None);
    let failed = RwSignal::new(false);
    let zoom = RwSignal::new(0u16); // 0 = Fit
    let root = NodeRef::<html::Section>::new();
    let index = siblings.iter().position(|s| s.id == entry.id);
    let previous = index.filter(|i| *i > 0).map(|i| siblings[i - 1].clone());
    let next = index.and_then(|i| siblings.get(i + 1).cloned());
    let opener = send_wrapper::SendWrapper::new(active_element());
    on_cleanup(move || {
        let Some(opener) = opener.take().filter(|o| o.is_connected()) else {
            return;
        };
        let active = crate::ui::document().active_element();
        let inside = root
            .get_untracked()
            .is_some_and(|r| r.contains(active.as_ref().map(|a| a.unchecked_ref())));
        if active.is_none() || active == crate::ui::document().body().map(Into::into) || inside {
            let _ = opener.focus();
        }
    });
    Effect::new(move |_| {
        if let Some(r) = root.get() {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = r.focus_with_options(&options);
        }
    });
    if kind == Some("text") {
        let source = source.clone();
        spawn_local(async move {
            match api::fetch(&source, Default::default()).await {
                Ok(response) if response.ok() => match response.text() {
                    Ok(promise) => match wasm_bindgen_futures::JsFuture::from(promise).await {
                        Ok(value) => {
                            text.try_set(value.as_string());
                        }
                        Err(_) => {
                            failed.try_set(true);
                        }
                    },
                    Err(_) => {
                        failed.try_set(true);
                    }
                },
                _ => {
                    failed.try_set(true);
                }
            }
        });
    }
    let label = file_type_label(&entry.kind, &entry.name, &entry.media_type);
    let (tile_kind, tile_name, tile_media) = (
        entry.kind.clone(),
        entry.name.clone(),
        entry.media_type.clone(),
    );
    let tile = move |large: bool| view! { <FileTile kind=tile_kind.clone() name=tile_name.clone() media_type=tile_media.clone() large=large/> };
    let unavailable = {
        let content = content.clone();
        let tile = tile.clone();
        move |failed: bool| {
            view! {
                <div class="empty-state file-preview-empty">{tile(true)}
                    <h2>{if failed { "This preview could not load" } else { "No preview for this file type" }}</h2>
                    <p>{if failed { "The file may have changed or is not readable by this browser.".to_owned() } else { format!("{label} files can’t be shown in the browser. Download the file to open it.") }}</p>
                    <a class="primary-button" href=content.clone() download=""><Icon name="download"/>"Download"</a>
                </div>
            }
        }
    };
    let unavailable = StoredValue::new(unavailable);
    let name = entry.name.clone();
    let entry_sv = StoredValue::new(entry.clone());
    let zoom_out = move || {
        zoom.update(|z| {
            let cur = ZOOM_STEPS.iter().position(|&v| v == *z).unwrap_or(0);
            *z = if cur == 0 {
                75
            } else {
                ZOOM_STEPS[cur.saturating_sub(1).max(1)]
            };
        });
    };
    let zoom_in = move || {
        zoom.update(|z| {
            let cur = ZOOM_STEPS.iter().position(|&v| v == *z).unwrap_or(0);
            *z = if cur == 0 {
                150
            } else {
                ZOOM_STEPS[(cur + 1).min(ZOOM_STEPS.len() - 1)]
            };
        });
    };
    let stage = {
        let (source, name) = (source.clone(), name.clone());
        let pdf_url = format!("{content}?v={}", entry.revision);
        let audio_tile = tile.clone();
        move || {
            if failed.get() || kind.is_none() {
                return unavailable.with_value(|u| u(failed.get())).into_any();
            }
            if kind == Some("pdf") {
                return view! { <crate::books::PdfView url=pdf_url.clone() label=format!("PDF {}", name) class_name="file-preview-document" on_error=Callback::new(move |_| failed.set(true))/> }.into_any();
            }
            match kind {
                Some("image") => view! {
                    <div class=move || format!("file-preview-image-wrap {}", zoom_class(zoom.get()))>
                        <img src=source.clone() alt=name.clone() on:error=move |_| failed.set(true)/>
                    </div>
                    <div class="file-preview-zoom" role="group" aria-label="Image zoom">
                        <button type="button" class="icon-button" aria-label="Zoom out" title="Zoom out (-)" disabled=move || zoom.get() == 50 on:click=move |_| zoom_out()>"−"</button>
                        <span class="file-preview-zoom-value">{move || match zoom.get() { 0 => "Fit".to_owned(), v => format!("{v}%") }}</span>
                        <button type="button" class="icon-button" aria-label="Zoom in" title="Zoom in (+)" disabled=move || zoom.get() == 200 on:click=move |_| zoom_in()><Icon name="plus"/></button>
                        <button type="button" class="text-button" aria-label="Fit to window" disabled=move || zoom.get() == 0 on:click=move |_| zoom.set(0)>"Fit"</button>
                    </div>
                }.into_any(),
                Some("video") => view! { <video src=source.clone() controls playsinline preload="metadata" aria-label=name.clone() on:error=move |_| failed.set(true)></video> }.into_any(),
                Some("audio") => view! { <div class="file-preview-audio">{audio_tile(true)}<audio src=source.clone() controls preload="metadata" aria-label=name.clone() on:error=move |_| failed.set(true)></audio></div> }.into_any(),
                _ => match text.get() {
                    None => view! { <p class="muted" role="status">"Loading preview…"</p> }.into_any(),
                    Some(t) => view! { <pre class="file-preview-source">{t}</pre> }.into_any(),
                },
            }
        }
    };
    let mut menu = vec![MenuAction::new("Open in new tab", "external").href(source.clone())];
    menu.extend(actions);
    let menu = StoredValue::new(menu);
    let count = siblings.len();
    let (prev_key, next_key) = (previous.clone(), next.clone());
    let base_class = format!(
        "file-text-editor document-editor file-preview file-preview-{}",
        kind.unwrap_or("none")
    );
    view! {
        <section node_ref=root tabindex="-1"
            class=move || if details_open.get() { format!("{base_class} has-details-beside") } else { base_class.clone() }
            aria-label=format!("Preview of {name}")
            on:keydown=move |e| {
                if e.default_prevented() { return; }
                let inside = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).and_then(|t| t.closest("input,textarea,video,audio,[role=menu]").ok().flatten());
                if inside.is_some() { return; }
                match e.key().as_str() {
                    "Escape" => { e.prevent_default(); { on_close.try_run(()); }; }
                    "ArrowLeft" => if let Some(p) = prev_key.clone() { e.prevent_default(); { on_open.try_run(p); }; },
                    "ArrowRight" => if let Some(n) = next_key.clone() { e.prevent_default(); { on_open.try_run(n); }; },
                    "+" | "=" if kind == Some("image") => { e.prevent_default(); zoom_in(); }
                    "-" if kind == Some("image") => { e.prevent_default(); zoom_out(); }
                    "0" if kind == Some("image") => { e.prevent_default(); zoom.set(0); }
                    _ => {}
                }
            }>
            <div class="workspace-editor-top">
                <button class="icon-button" aria-label="Close preview" title="Close (Escape)" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button>
                <span class="note-notebook">{tile(false)}{label}</span>
                <span class="file-preview-meta">{format_size(entry.size)}" · Edited "{edited(entry.modified_at)}</span>
                {(count > 1 && index.is_some()).then(|| {
                    let (p, n) = (previous.clone(), next.clone());
                    let (p2, n2) = (previous.is_none(), next.is_none());
                    view! {
                        <div class="file-preview-steps" role="group" aria-label="Browse files">
                            <button class="icon-button" aria-label="Previous file" title="Previous (←)" disabled=p2 on:click=move |_| if let Some(p) = p.clone() { { on_open.try_run(p); } }><Icon name="back"/></button>
                            <span>{format!("{} of {count}", index.unwrap_or(0) + 1)}</span>
                            <button class="icon-button" aria-label="Next file" title="Next (→)" disabled=n2 on:click=move |_| if let Some(n) = n.clone() { { on_open.try_run(n); } }><Icon name="chevron"/></button>
                        </div>
                    }
                })}
                {on_share.map(|cb| view! {
                    <button class="secondary-button file-preview-share" aria-label="Share file" title="Share" on:click=move |_| { cb.try_run(entry_sv.get_value()); }><Icon name="share"/><span>"Share"</span></button>
                })}
                <a class="icon-button" href=content.clone() download="" aria-label="Download" title="Download"><Icon name="download"/></a>
                {on_toggle_details.map(|cb| view! {
                    <button class=move || if details_open.get() { "icon-button is-active" } else { "icon-button" } aria-label="Toggle file details" title="Details" aria-pressed=move || details_open.get().to_string() on:click=move |_| { cb.try_run(entry_sv.get_value()); }><Icon name="info"/></button>
                })}
                <ActionMenu label="File options" actions=move |_| menu.get_value()><Icon name="more"/></ActionMenu>
            </div>
            <h1 class="file-preview-title">{name.clone()}</h1>
            <div class="file-preview-stage">{stage}</div>
        </section>
    }
}
