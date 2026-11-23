//! Repeat and reminder fields (`pim/schedule-controls.tsx`).

use leptos::prelude::*;

use super::presentation::recurrence_label;
use crate::ui::Select;

const REPEATS: [&str; 4] = ["DAILY", "WEEKLY", "MONTHLY", "YEARLY"];

#[component]
pub fn RepeatField(value: RwSignal<String>) -> impl IntoView {
    let custom = RwSignal::new(false);
    let mode = Memo::new(move |_| {
        let v = value.get();
        let simple = v.is_empty() || REPEATS.iter().any(|f| v == format!("FREQ={f}"));
        if custom.get() || !simple { "custom".to_owned() } else { v.trim_start_matches("FREQ=").to_lowercase() }
    });
    let selected = RwSignal::new(mode.get_untracked());
    Effect::new(move |_| selected.set(mode.get()));
    let options: Vec<(String, String)> = [("", "Does not repeat"), ("daily", "Daily"), ("weekly", "Weekly"), ("monthly", "Monthly"), ("yearly", "Yearly"), ("custom", "Custom schedule…")].iter().map(|(v, l)| ((*v).into(), (*l).into())).collect();
    view! {
        <div>
            <input type="hidden" name="recurrence" prop:value=move || value.get()/>
            <label>"Repeats"<Select aria_label="Repeats" options=options value=selected on_change=Callback::new(move |next: String| {
                custom.set(next == "custom");
                if next != "custom" { value.set(if next.is_empty() { String::new() } else { format!("FREQ={}", next.to_uppercase()) }); }
                else if value.get_untracked().is_empty() { value.set("FREQ=WEEKLY;COUNT=10".into()); }
            })/></label>
            {move || (!value.get().is_empty()).then(|| view! { <p class="field-hint">{recurrence_label(&value.get(), false)}</p> })}
            {move || (mode.get() == "custom").then(|| view! {
                <details open=custom.get_untracked() on:toggle=move |e: web_sys::Event| { if let Some(d) = e.target().and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlDetailsElement>(t).ok()) { custom.set(d.open()); } }>
                    <summary>"Custom recurrence"</summary>
                    <label>"Recurrence rule"<input aria-label="Recurrence rule" prop:value=move || value.get() maxlength="2048" on:input=move |e| value.set(event_target_value(&e))/><small>"For advanced schedules, for example FREQ=WEEKLY;BYDAY=MO,WE;COUNT=12."</small></label>
                </details>
            })}
        </div>
    }
}

const REMINDERS: [(&str, &str); 9] = [("", "No reminder"), ("0", "At start"), ("5", "5 minutes before"), ("10", "10 minutes before"), ("15", "15 minutes before"), ("30", "30 minutes before"), ("60", "1 hour before"), ("1440", "1 day before"), ("10080", "1 week before")];

/// `value` is minutes before the start; empty means no reminder.
#[component]
pub fn ReminderField(value: RwSignal<String>) -> impl IntoView {
    let custom = RwSignal::new(false);
    let mode = Memo::new(move |_| {
        let v = value.get();
        if custom.get() || !REMINDERS.iter().any(|(o, _)| *o == v) { "custom".to_owned() } else { v }
    });
    let selected = RwSignal::new(mode.get_untracked());
    Effect::new(move |_| selected.set(mode.get()));
    let mut options: Vec<(String, String)> = REMINDERS.iter().map(|(v, l)| ((*v).into(), (*l).into())).collect();
    options.push(("custom".into(), "Custom reminder…".into()));
    view! {
        <div>
            <input type="hidden" name="reminderMinutes" prop:value=move || value.get()/>
            <label>"Reminder"<Select aria_label="Reminder" options=options value=selected on_change=Callback::new(move |next: String| {
                custom.set(next == "custom");
                if next != "custom" { value.set(next); } else if value.get_untracked().is_empty() { value.set("20".into()); }
            })/></label>
            {move || (mode.get() == "custom").then(|| view! { <label>"Minutes before"<input type="number" min="0" max="10080" prop:value=move || value.get() on:input=move |e| value.set(event_target_value(&e))/></label> })}
        </div>
    }
}
