//! Regional settings (`regional.tsx`): timezone preference.

use std::collections::BTreeSet;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::BrowserSession;
use crate::settings::{SaveProblems, SaveState, Saver};
use crate::ui::Select;

fn supported_timezones(current: &str) -> Vec<(String, String)> {
    let mut set = BTreeSet::new();
    set.insert("UTC".to_owned());
    if !current.is_empty() {
        set.insert(current.to_owned());
    }
    if let Ok(intl) = js_sys::Reflect::get(&js_sys::global(), &"Intl".into())
        && let Ok(func) = js_sys::Reflect::get(&intl, &"supportedValuesOf".into())
        && let Some(func) = func.dyn_ref::<js_sys::Function>()
        && let Ok(values) = func.call1(&intl, &"timeZone".into())
    {
        let arr = js_sys::Array::from(&values);
        for i in 0..arr.length() {
            if let Some(zone) = arr.get(i).as_string() {
                set.insert(zone);
            }
        }
    }
    set.into_iter()
        .map(|z| {
            let label = z.replace('_', " ");
            (z, label)
        })
        .collect()
}

/// Regional settings section (`RegionalSettings`).
#[component]
pub fn RegionalSettings(session: BrowserSession) -> impl IntoView {
    let saver = Saver::new(session);
    let loaded = Memo::new(move |_| saver.preferences.with(Option::is_some));
    let zone = RwSignal::new("UTC".to_owned());
    Effect::new(move |_| {
        let next = saver.value("timezone").as_str().unwrap_or("UTC").to_owned();
        if zone.get_untracked() != next {
            zone.set(next);
        }
    });
    let state = saver.state("timezone");
    view! {
        <h1>"Regional settings"</h1>
        <SaveProblems saver=saver/>
        {move || {
            if !loaded.get() {
                view! { <p role="status">"Loading regional settings\u{2026}"</p> }.into_any()
            } else {
                let options = supported_timezones(&zone.get_untracked());
                view! {
                    <div class="ui-profile-form">
                        <label>
                            "Timezone"
                            <Select
                                aria_label="Timezone"
                                options=options
                                value=zone
                                on_change=Callback::new(move |next: String| {
                                    saver.change("timezone", serde_json::json!(next), "Regional settings saved".into(), false);
                                })
                            />
                        </label>
                        <p class="field-hint">"Used for calendar display, task dates and your reminders. Saves as you choose."</p>
                        <p role="status" class="settings-inline-status">
                            {move || match state.get() {
                                Some(SaveState::Saving) => "Saving\u{2026}",
                                Some(SaveState::Saved) => "Regional settings saved",
                                _ => "",
                            }}
                        </p>
                    </div>
                }
                .into_any()
            }
        }}
    }
}
