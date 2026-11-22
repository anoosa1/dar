//! Administration → Appearance (`instance-appearance.tsx`): sign-in logo customization.

use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::api::{self, BrowserSession, Req};
use crate::app::Brand;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceAppearance {
    pub revision: u64,
    #[serde(default)]
    pub logo_url: Option<String>,
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Administration → Appearance section (`InstanceAppearanceSettings`).
#[component]
pub fn InstanceAppearanceSettings(session: BrowserSession) -> impl IntoView {
    let appearance: RwSignal<Option<InstanceAppearance>> = RwSignal::new(None);
    let preview: RwSignal<Option<String>> = RwSignal::new(None);
    let file: RwSignal<Option<SendWrapper<web_sys::File>>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let saved = RwSignal::new(false);
    let csrf = StoredValue::new(session.csrf_token);

    if let Ok(controller) = web_sys::AbortController::new() {
        let signal = controller.signal();
        let abort_ref = SendWrapper::new(controller);
        let signal_check = SendWrapper::new(signal.clone());
        on_cleanup(move || abort_ref.take().abort());
        spawn_local(async move {
            match api::request::<InstanceAppearance>(
                "/api/v1/appearance",
                Req { signal: Some(signal), ..Default::default() },
            )
            .await
            {
                Ok(value) => {
                    let _ = preview.try_set(value.logo_url.clone());
                    let _ = appearance.try_set(Some(value));
                }
                Err(e) => {
                    if !signal_check.aborted() {
                        let _ = error.try_set(e.message);
                    }
                }
            }
        });
    }

    // Revoke any object URL created for a locally chosen file when replaced or unmounted.
    Effect::new(move |_| {
        let Some(selected) = file.get() else { return };
        if let Ok(url) = web_sys::Url::create_object_url_with_blob(&selected) {
            preview.set(Some(url.clone()));
            on_cleanup(move || {
                let _ = web_sys::Url::revoke_object_url(&url);
            });
        }
    });

    let on_submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(current) = appearance.get_untracked() else { return };
        busy.set(true);
        error.set(String::new());
        saved.set(false);
        let selected = file.get_untracked();
        let current_preview = preview.get_untracked();
        let token = csrf.get_value();
        spawn_local(async move {
            let (logo, media_type) = if let Some(f) = selected {
                let mt = f.type_();
                match JsFuture::from(f.array_buffer()).await {
                    Ok(buf) => {
                        let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                        (encode_base64(&bytes), mt)
                    }
                    Err(_) => {
                        let _ = error.try_set("The selected file could not be read.".to_owned());
                        let _ = busy.try_set(false);
                        return;
                    }
                }
            } else if current_preview.is_some() {
                let _ = busy.try_set(false);
                let _ = saved.try_set(true);
                return;
            } else {
                (String::new(), String::new())
            };

            let etag = format!("\"appearance:{}\"", current.revision);
            let body = json!({
                "revision": current.revision,
                "logo": logo,
                "mediaType": media_type,
            });
            match api::request::<InstanceAppearance>(
                "/api/v1/appearance",
                Req {
                    method: "PUT",
                    body: Some(body.to_string()),
                    csrf: Some(&token),
                    headers: &[("if-match", &etag)],
                    ..Default::default()
                },
            )
            .await
            {
                Ok(value) => {
                    let _ = file.try_set(None);
                    let _ = preview.try_set(value.logo_url.clone());
                    let _ = appearance.try_set(Some(value));
                    let _ = saved.try_set(true);
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let on_file_change = move |event: web_sys::Event| {
        let Some(input) = event.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) else {
            return;
        };
        saved.set(false);
        let Some(selected) = input.files().and_then(|list| list.get(0)) else {
            return;
        };
        let mime = selected.type_();
        if !matches!(mime.as_str(), "image/png" | "image/webp") || selected.size() > (256 * 1024) as f64 {
            error.set("Choose a PNG or WebP image up to 256 KiB.".to_owned());
            input.set_value("");
            return;
        }
        error.set(String::new());
        file.set(Some(SendWrapper::new(selected)));
    };

    let loaded = Memo::new(move |_| appearance.with(Option::is_some));

    view! {
        <h1>"Appearance"</h1>
        <p class="muted">"Customize the sign-in logo for everyone on this instance."</p>
        {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
        {move || if !loaded.get() {
            view! { <p role="status">"Loading appearance\u{2026}"</p> }.into_any()
        } else {
            view! {
                <form on:submit=on_submit>
                    <h2>"Sign-in logo"</h2>
                    <div class="signin-logo-preview">
                        {move || match preview.get() {
                            Some(src) => view! { <img src=src alt="Sign-in logo preview"/> }.into_any(),
                            None => view! { <Brand/> }.into_any(),
                        }}
                    </div>
                    <label>
                        "Upload logo"
                        <input
                            type="file"
                            accept="image/png,image/webp"
                            disabled=move || busy.get()
                            on:change=on_file_change
                        />
                    </label>
                    <p class="field-hint">"PNG or WebP, up to 256 KiB. Transparent images work best."</p>
                    <button
                        class="secondary-button"
                        type="button"
                        disabled=move || busy.get()
                        on:click=move |_| {
                            file.set(None);
                            preview.set(None);
                            saved.set(false);
                        }
                    >
                        "Use Dar logo"
                    </button>
                    <div class="dialog-actions">
                        <button class="primary-button" disabled=move || busy.get()>
                            {move || if busy.get() { "Saving\u{2026}" } else { "Save appearance" }}
                        </button>
                    </div>
                    {move || saved.get().then(|| view! { <p role="status">"Appearance saved"</p> })}
                </form>
            }.into_any()
        }}
    }
}
