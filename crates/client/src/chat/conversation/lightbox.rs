//! Portal modal media lightbox for viewing, downloading, and saving chat photos.

use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::super::{ChatMessage, LightboxItem};
use crate::icons::Icon;
use crate::ui::{active_element, document, listen};

/// Fullscreen modal lightbox rendered in a portal with focus capture and Escape dismissal.
#[component]
pub fn MediaLightbox(
    item: LightboxItem,
    can_save: bool,
    on_save: Callback<ChatMessage>,
    on_close: Callback<()>,
) -> impl IntoView {
    let backdrop_ref = NodeRef::<html::Div>::new();
    let dialog_ref = NodeRef::<html::Div>::new();
    let previous = send_wrapper::SendWrapper::new(active_element());

    on_cleanup(move || {
        if let Some(el) = previous
            .take()
            .filter(|p| p.is_connected() && p.get_bounding_client_rect().width() > 0.0)
        {
            let _ = el.focus();
        }
    });

    Effect::new(move |_| {
        if let Some(dialog) = dialog_ref.get() {
            let opts = web_sys::FocusOptions::new();
            opts.set_prevent_scroll(true);
            let _ = dialog.focus_with_options(&opts);
        }
    });

    listen(&document(), "keydown", false, move |ev| {
        let e: web_sys::KeyboardEvent = ev.unchecked_into();
        if e.key() == "Escape" && !e.default_prevented() {
            e.prevent_default();
            e.stop_propagation();
            on_close.try_run(());
        }
    });

    let alt_text = if !item.caption.is_empty() {
        item.caption.clone()
    } else if !item.file_name.is_empty() {
        item.file_name.clone()
    } else {
        "Photo".to_owned()
    };
    let sender = StoredValue::new(item.sender);
    let time_opt = StoredValue::new((!item.time.is_empty()).then_some(item.time));
    let caption_opt = StoredValue::new((!item.caption.is_empty()).then_some(item.caption));
    let save_message = StoredValue::new(item.message.filter(|_| can_save));
    let url = StoredValue::new(item.url);
    let download_url = StoredValue::new(item.download_url);
    let file_name = StoredValue::new(item.file_name);
    let alt_text = StoredValue::new(alt_text);

    view! {
        <Portal>
            <div
                node_ref=backdrop_ref
                class="chat-lightbox-backdrop"
                on:click=move |e: web_sys::MouseEvent| {
                    if let (Some(target), Some(backdrop)) = (e.target(), backdrop_ref.get_untracked()) {
                        let backdrop_target: &web_sys::EventTarget = backdrop.unchecked_ref();
                        if &target == backdrop_target {
                            on_close.try_run(());
                        }
                    }
                }
            >
                <div
                    node_ref=dialog_ref
                    class="chat-lightbox"
                    role="dialog"
                    aria-modal="true"
                    aria-label="Media preview"
                    tabindex="-1"
                >
                    <header class="chat-lightbox-head">
                        <div>
                            <b>{sender.get_value()}</b>
                            {time_opt.get_value().map(|time| view! { <small>{time}</small> })}
                        </div>
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Close preview"
                            on:click=move |_| { on_close.try_run(()); }
                        >
                            <Icon name="close"/>
                        </button>
                    </header>
                    <div class="chat-lightbox-stage">
                        <img src=url.get_value() alt=alt_text.get_value()/>
                    </div>
                    <footer class="chat-lightbox-foot">
                        {caption_opt.get_value().map(|caption| view! {
                            <span class="chat-lightbox-caption">{caption}</span>
                        })}
                        <div class="chat-lightbox-actions">
                            {save_message.get_value().map(|msg| view! {
                                <button
                                    type="button"
                                    class="secondary-button"
                                    on:click=move |_| {
                                        on_close.try_run(());
                                        on_save.try_run(msg.clone());
                                    }
                                >
                                    <Icon name="folder"/>"Save to Files"
                                </button>
                            })}
                            <a
                                class="primary-button"
                                href=download_url.get_value()
                                download=file_name.get_value()
                            >
                                <Icon name="download"/>"Download"
                            </a>
                        </div>
                    </footer>
                </div>
            </div>
        </Portal>
    }
}
