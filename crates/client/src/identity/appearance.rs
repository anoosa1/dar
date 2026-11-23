//! Account theme hook and Appearance settings (`appearance.tsx`).

use leptos::prelude::*;

use crate::api::BrowserSession;
use crate::settings::{SaveProblems, SaveState, Saver};
use crate::shared::Context;
use crate::ui::{listen, window};

const APPEARANCES: [(&str, &str); 3] = [
    ("system", "Follow system"),
    ("latte", "Light \u{2014} Twilight Afterglow"),
    ("mocha", "Dark \u{2014} Twilight Nightfall"),
];

fn apply_theme(value: &str) {
    let dark = window()
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches());
    let theme = if value == "system" {
        if dark { "mocha" } else { "latte" }
    } else {
        value
    };
    if let Some(root) = window().document().and_then(|d| d.document_element()) {
        let _ = root.set_attribute("data-theme", theme);
    }
}

/// Applies the account's appearance preference (or system theme when signed out or `"system"`)
/// to `document.documentElement.dataset.theme` and tracks OS theme changes.
pub fn use_appearance(session: Signal<Option<BrowserSession>>) {
    let current = RwSignal::new("system".to_owned());
    let shared = use_context::<Context>();
    Effect::new(move |_| {
        let user_id = session.with(|s| s.as_ref().map(|s| s.user.id.clone()));
        let mut value = user_id
            .as_deref()
            .and_then(|id| {
                let key = format!("dar.appearance.{id}");
                window().local_storage().ok().flatten()?.get_item(&key).ok().flatten()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "system".to_owned());
        if let (Some(id), Some(ctx)) = (user_id.as_deref(), shared)
            && let Some(p) = ctx.preferences.value.get()
        {
            value = p.appearance.unwrap_or_else(|| "system".to_owned());
            if let Some(s) = window().local_storage().ok().flatten() {
                let _ = s.set_item(&format!("dar.appearance.{id}"), &value);
            }
        }
        current.set(value.clone());
        apply_theme(&value);
    });
    if let Ok(Some(media)) = window().match_media("(prefers-color-scheme: dark)") {
        listen(&media, "change", false, move |_| {
            apply_theme(&current.get_untracked());
        });
    }
}

/// Personal Appearance settings section (`AppearanceSettings`).
#[component]
pub fn AppearanceSettings(session: BrowserSession) -> impl IntoView {
    let saver = Saver::new(session);
    let loaded = Memo::new(move |_| saver.preferences.with(Option::is_some));
    let value = Signal::derive(move || {
        saver
            .value("appearance")
            .as_str()
            .unwrap_or("system")
            .to_owned()
    });
    let state = saver.state("appearance");
    view! {
        <h1>"Appearance"</h1>
        <SaveProblems saver=saver/>
        {move || {
            if !loaded.get() {
                view! { <p role="status">"Loading appearance\u{2026}"</p> }.into_any()
            } else {
                view! {
                    <fieldset class="appearance-options">
                        <legend>"Color theme"</legend>
                        {APPEARANCES.into_iter().map(|(id, label)| {
                            view! {
                                <label class=move || if value.get() == id { "appearance-option is-checked" } else { "appearance-option" }>
                                    <input
                                        type="radio"
                                        name="appearance"
                                        value=id
                                        prop:checked=move || value.get() == id
                                        on:change=move |_| saver.change("appearance", serde_json::json!(id), "Appearance saved".into(), false)
                                    />
                                    <span>{label}</span>
                                    <span class=format!("theme-swatch swatch-{id}") aria-hidden="true"></span>
                                </label>
                            }
                        }).collect_view()}
                    </fieldset>
                    <p class="field-hint">"Applies to every app and saves for your account as you choose."</p>
                    <p role="status" class="settings-inline-status">
                        {move || match state.get() {
                            Some(SaveState::Saving) => "Saving\u{2026}",
                            Some(SaveState::Saved) => "Appearance saved",
                            _ => "",
                        }}
                    </p>
                }
                .into_any()
            }
        }}
    }
}
