//! Personal profile settings (`profile.tsx`).

use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use crate::api::{self, BrowserSession, Req};
use crate::shell::Avatar;
use crate::ui::window;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileName {
    #[serde(default)]
    given_name: Option<String>,
    #[serde(default)]
    family_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct ProfileEmail {
    value: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    id: String,
    user_name: String,
    display_name: String,
    revision: u64,
    #[serde(default)]
    name: Option<ProfileName>,
    #[serde(default)]
    emails: Vec<ProfileEmail>,
}

/// Personal profile settings section (`ProfileSettings`).
#[component]
pub fn ProfileSettings(session: BrowserSession) -> impl IntoView {
    let profile: RwSignal<Option<Profile>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let csrf = StoredValue::new(session.csrf_token.clone());
    let display_name = StoredValue::new(session.user.display_name.clone());

    if let Ok(controller) = web_sys::AbortController::new() {
        let signal = controller.signal();
        let abort_ref = SendWrapper::new(controller);
        let signal_check = SendWrapper::new(signal.clone());
        on_cleanup(move || abort_ref.take().abort());
        spawn_local(async move {
            match api::request::<Profile>(
                "/api/v1/account/profile",
                Req {
                    signal: Some(signal),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(value) => {
                    let _ = profile.try_set(Some(value));
                }
                Err(e) => {
                    if !signal_check.aborted() {
                        let _ = error.try_set(e.message);
                    }
                }
            }
        });
    }

    let loaded = Memo::new(move |_| profile.with(Option::is_some));

    view! {
        <h1>"Profile"</h1>
        <p class="muted">"Your account name and contact details."</p>
        {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
        {move || {
            if !loaded.get() {
                return view! { <p role="status">"Loading profile\u{2026}"</p> }.into_any();
            }
            let Some(p) = profile.get_untracked() else {
                return view! { <p role="status">"Loading profile\u{2026}"</p> }.into_any();
            };
            let primary_email = p
                .emails
                .iter()
                .find(|e| e.primary)
                .or_else(|| p.emails.first())
                .map(|e| e.value.clone())
                .unwrap_or_default();
            let given_name = p.name.as_ref().and_then(|n| n.given_name.clone()).unwrap_or_default();
            let family_name = p.name.as_ref().and_then(|n| n.family_name.clone()).unwrap_or_default();
            let etag = format!("\"{}:{}\"", p.id, p.revision);
            let on_submit = move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                let Some(form) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok()) else {
                    return;
                };
                let Ok(data) = web_sys::FormData::new_with_form(&form) else { return };
                let mut body = serde_json::Map::new();
                for name in ["displayName", "givenName", "familyName", "email"] {
                    if let Some(value) = data.get(name).as_string() {
                        body.insert(name.into(), value.into());
                    }
                }
                busy.set(true);
                error.set(String::new());
                let etag = etag.clone();
                spawn_local(async move {
                    let result = api::request::<serde_json::Value>(
                        "/api/v1/account/profile",
                        Req {
                            method: "PUT",
                            body: Some(serde_json::Value::Object(body).to_string()),
                            csrf: Some(&csrf.get_value()),
                            headers: &[("if-match", &etag)],
                            ..Default::default()
                        },
                    )
                    .await;
                    match result {
                        Ok(_) => {
                            let _ = window().location().reload();
                        }
                        Err(err) => {
                            let _ = error.try_set(err.message);
                        }
                    }
                    let _ = busy.try_set(false);
                });
            };
            view! {
                <form class="ui-profile-form" on:submit=on_submit>
                    <Avatar name=display_name.get_value()/>
                    <label>
                        "Username"
                        <input readonly=true value=p.user_name/>
                    </label>
                    <p class="field-hint">"Your username is the stable sign-in identity. It cannot be changed here."</p>
                    <label>
                        "Display name"
                        <input name="displayName" required maxlength="100" value=p.display_name/>
                    </label>
                    <div class="identity-form-grid">
                        <label>
                            "Given name"
                            <input name="givenName" maxlength="100" value=given_name/>
                        </label>
                        <label>
                            "Family name"
                            <input name="familyName" maxlength="100" value=family_name/>
                        </label>
                    </div>
                    <label>
                        "Primary email"
                        <input type="email" name="email" maxlength="254" value=primary_email/>
                    </label>
                    <p class="field-hint">"Email addresses are not marked verified."</p>
                    <button class="primary-button" disabled=move || busy.get()>
                        {move || if busy.get() { "Saving\u{2026}" } else { "Save changes" }}
                    </button>
                </form>
            }
            .into_any()
        }}
    }
}
