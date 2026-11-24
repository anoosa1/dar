//! The account's Locked folder for this browser session (`locked-folder.tsx`).

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;

use crate::api::{self, BrowserSession, RequestError};
use crate::icons::Icon;
use crate::ui::Dialog;

const ENDPOINT: &str = "/api/v1/files/locked";
/// How long one password confirmation keeps a browser session unlocked.
const UNLOCK_MINUTES: u32 = 10;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedState {
    pub folder_id: String,
    pub unlocked_until: f64,
}

/// Relocks by itself when the unlock expires.
#[derive(Clone, Copy)]
pub struct LockedFolder {
    pub state: RwSignal<Option<LockedState>>,
    pub error: RwSignal<String>,
    session: StoredValue<BrowserSession>,
    timer: RwSignal<u32>,
}

impl LockedFolder {
    pub fn new(session: BrowserSession) -> Self {
        let folder = Self {
            state: RwSignal::new(None),
            error: RwSignal::new(String::new()),
            session: StoredValue::new(session),
            timer: RwSignal::new(0),
        };
        folder.refresh();
        folder
    }

    pub fn unlocked(self) -> bool {
        self.state.with(|s| {
            s.as_ref()
                .is_some_and(|s| s.unlocked_until > js_sys::Date::now())
        })
    }

    fn accept(self, value: LockedState) {
        let id = self.timer.get_untracked() + 1;
        self.timer.set(id);
        let wait = value.unlocked_until - js_sys::Date::now();
        if value.unlocked_until > 0.0 {
            spawn_local(async move {
                crate::sleep((wait.max(0.0) + 500.0) as u32).await;
                if self.timer.try_get_untracked() == Some(id) {
                    self.refresh();
                }
            });
        }
        self.state.set(Some(value));
        self.error.set(String::new());
    }

    pub fn refresh(self) {
        spawn_local(async move {
            match api::get::<LockedState>(ENDPOINT).await {
                Ok(value) => self.accept(value),
                Err(e) => self.error.set(e.message),
            }
        });
    }

    pub async fn unlock(self, password: String) -> Result<LockedState, RequestError> {
        let csrf = self.session.with_value(|s| s.csrf_token.clone());
        let value: LockedState = api::send(
            &format!("{ENDPOINT}/unlock"),
            "POST",
            Some(serde_json::json!({ "password": password })),
            &csrf,
        )
        .await?;
        self.accept(value.clone());
        Ok(value)
    }

    pub async fn lock(self) -> Result<LockedState, RequestError> {
        let csrf = self.session.with_value(|s| s.csrf_token.clone());
        let value: LockedState =
            api::send(&format!("{ENDPOINT}/lock"), "POST", None, &csrf).await?;
        self.accept(value.clone());
        Ok(value)
    }
}

/// The password form shown in place of locked content.
#[component]
pub fn UnlockForm(
    folder: LockedFolder,
    #[prop(into)] description: String,
    #[prop(optional)] compact: bool,
    #[prop(into)] on_unlocked: Callback<LockedState>,
) -> impl IntoView {
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let input = NodeRef::<html::Input>::new();
    if !compact {
        Effect::new(move |_| {
            if let Some(i) = input.get() {
                let options = web_sys::FocusOptions::new();
                options.set_prevent_scroll(true);
                let _ = i.focus_with_options(&options);
            }
        });
    }
    let submit = move |e: web_sys::SubmitEvent| {
        e.prevent_default();
        let value = password.get_untracked();
        if value.is_empty() {
            error.set("Enter your password.".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        spawn_local(async move {
            match folder.unlock(value).await {
                Ok(state) => {
                    password.set(String::new());
                    {
                        on_unlocked.try_run(state);
                    };
                }
                Err(e) => {
                    error.set(e.message);
                    if let Some(i) = input.get_untracked() {
                        let _ = i.focus();
                    }
                }
            }
            busy.set(false);
        });
    };
    view! {
        <form class=if compact { "locked-unlock is-compact" } else { "locked-unlock" } on:submit=submit>
            {(!compact).then(|| view! { <span class="locked-unlock-icon" aria-hidden="true"><Icon name="lock"/></span><h2>"Locked folder"</h2> })}
            <p>{description}</p>
            <label>"Password"<input node_ref=input type="password" autocomplete="current-password" prop:value=move || password.get()
                on:input=move |e| { password.set(event_target_value(&e)); error.set(String::new()); }
                aria-invalid=move || (!error.get().is_empty()).to_string() aria-describedby=move || (!error.get().is_empty()).then_some("locked-unlock-error")/></label>
            {move || (!error.get().is_empty()).then(|| view! { <p id="locked-unlock-error" role="alert" class="error">{error.get()}</p> })}
            <button class="primary-button" disabled=move || busy.get()><Icon name="lock"/>{move || if busy.get() { "Unlocking…" } else { "Unlock" }}</button>
            <small class="field-hint">"Stays open on this device for "{UNLOCK_MINUTES}" minutes, or until you lock it."</small>
        </form>
    }
}

/// Asks for the password before an action that needs the Locked folder, then runs it.
#[component]
pub fn UnlockDialog(
    folder: LockedFolder,
    #[prop(into)] action: String,
    on_close: Callback<()>,
    on_unlocked: Callback<LockedState>,
) -> impl IntoView {
    let description = format!("Enter your password to {action}.");
    view! {
        <Dialog title="Unlock the Locked folder" on_close=on_close>
            <UnlockForm compact=true folder=folder description=description.clone() on_unlocked=on_unlocked/>
        </Dialog>
    }
}
